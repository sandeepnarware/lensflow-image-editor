use crate::{AccountError, Instance};
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, KeyInit, Nonce};
use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

const SERVICE: &str = "lensflow-ImageEditor";

/// The token file format: magic, a random 12-byte nonce, then ChaCha20-Poly1305 ciphertext
/// and tag (the base URL is the associated data).
const MAGIC: &[u8; 4] = b"LFT1";
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
/// Random HKDF salt, made once per config dir and shared by every base URL's token file.
const SALT_FILE: &str = "token-salt";
const SALT_LEN: usize = 16;
const KEY_INFO: &str = "lensflow-ImageEditor token v1";
const UNREADABLE: &str = "the saved sign-in could not be read on this machine; sign in again";

/// Where the token file's key comes from: bytes bound to this machine and user account.
/// The default is [`MachineKey`]; tests pass their own to simulate another machine.
/// Never log these bytes.
pub trait KeySource: Send + Sync {
    fn key_material(&self) -> Vec<u8>;
}

/// The machine id, the user id and the user name (see the README's token-store notes).
pub struct MachineKey;

impl KeySource for MachineKey {
    fn key_material(&self) -> Vec<u8> {
        static MATERIAL: OnceLock<Vec<u8>> = OnceLock::new();
        MATERIAL.get_or_init(machine_key_material).clone()
    }
}

/// The sign-in token: OS keychain first; when there is none, a 0600 file under the config
/// dir, encrypted with a key bound to this machine and user account.
pub struct TokenStore {
    account: String,
    dir: PathBuf,
    use_keyring: bool,
    key: Arc<dyn KeySource>,
}

impl TokenStore {
    pub fn open(instance: &Instance) -> TokenStore {
        let project = directories::ProjectDirs::from("in", "lensflow", "lensflow-ImageEditor")
            .map(|d| d.config_dir().to_path_buf());
        Self::open_in(instance.base_url().as_str(), &config_dir(project), true)
    }

    /// For tests and the app's own config dir: `use_keyring = false` keeps everything in the file.
    pub fn open_in(base_url: &str, dir: &Path, use_keyring: bool) -> TokenStore {
        Self::open_with_key(base_url, dir, use_keyring, Arc::new(MachineKey))
    }

    /// As [`TokenStore::open_in`], with the file key taken from `key` instead of this machine.
    pub fn open_with_key(
        base_url: &str,
        dir: &Path,
        use_keyring: bool,
        key: Arc<dyn KeySource>,
    ) -> TokenStore {
        TokenStore {
            account: base_url.to_string(),
            dir: dir.to_path_buf(),
            use_keyring,
            key,
        }
    }

    fn entry(&self) -> Option<keyring::Entry> {
        self.entry_for(&self.account)
    }

    fn entry_for(&self, account: &str) -> Option<keyring::Entry> {
        if !self.use_keyring {
            return None;
        }
        match keyring::Entry::new(SERVICE, account) {
            Ok(e) => Some(e),
            Err(err) => {
                static WARNED: std::sync::Once = std::sync::Once::new();
                WARNED.call_once(|| {
                    log::warn!(
                        "the OS keychain is not available ({err}); the sign-in token is kept in an encrypted private file"
                    )
                });
                None
            }
        }
    }

    fn file(&self) -> PathBuf {
        let hash = hex::encode(Sha256::digest(self.account.as_bytes()));
        self.dir.join(format!("token-{}", &hash[..16]))
    }

    fn salt_file(&self) -> PathBuf {
        self.dir.join(SALT_FILE)
    }

    /// True when the keychain accepts a write and a read back (tests use it to skip cleanly).
    pub fn keyring_works(&self) -> bool {
        // A separate account so the probe never touches a stored token.
        let Some(e) = self.entry_for(&format!("{}#probe", self.account)) else {
            return false;
        };
        match e.set_password("probe") {
            Ok(()) => {
                let ok = e.get_password().is_ok();
                let _ = e.delete_credential();
                ok
            }
            Err(_) => false,
        }
    }

    pub fn load(&self) -> Option<String> {
        if let Some(e) = self.entry() {
            match e.get_password() {
                Ok(t) if !t.is_empty() => return Some(t),
                Ok(_) | Err(keyring::Error::NoEntry) => {}
                Err(err) => log::debug!("keychain read failed ({err}); trying the token file"),
            }
        }
        let path = self.file();
        let bytes = fs::read(&path).ok()?;
        if let Some(sealed) = bytes.strip_prefix(MAGIC) {
            return match self.open_sealed(sealed) {
                Some(t) => {
                    log::info!("using the sign-in token file (no keychain entry)");
                    Some(t)
                }
                None => {
                    log::warn!("{UNREADABLE}");
                    let _ = fs::remove_file(&path);
                    None
                }
            };
        }
        let Some(t) = legacy_token(&bytes) else {
            if !bytes.is_empty() {
                log::warn!("{UNREADABLE}");
            }
            let _ = fs::remove_file(&path);
            return None;
        };
        // A plaintext file from an older build: keep the sign-in, store it encrypted (or in
        // the keychain, when that works now) and never leave the plaintext behind.
        static MIGRATED: std::sync::Once = std::sync::Once::new();
        MIGRATED.call_once(|| log::info!("moving the saved sign-in to the encrypted token file"));
        self.save(&t);
        if fs::read(&path).is_ok_and(|b| !b.starts_with(MAGIC)) {
            let _ = fs::remove_file(&path);
        }
        Some(t)
    }

    pub fn save(&self, token: &str) {
        if let Some(e) = self.entry() {
            match e.set_password(token) {
                Ok(()) => {
                    let _ = fs::remove_file(self.file());
                    return;
                }
                Err(err) => {
                    log::warn!(
                        "no usable keychain ({err}); keeping the token in an encrypted private file"
                    )
                }
            }
        }
        if let Err(err) = self.write_file(token) {
            log::error!("could not store the sign-in token: {err}");
        }
    }

    /// HKDF-SHA256(ikm = key material, salt, info = KEY_INFO ‖ base URL) → ChaCha20-Poly1305.
    fn cipher(&self, salt: &[u8; SALT_LEN]) -> ChaCha20Poly1305 {
        let ikm = self.key.key_material();
        let info = [KEY_INFO.as_bytes(), self.account.as_bytes()].concat();
        let mut okm = [0u8; 32];
        Hkdf::<Sha256>::new(Some(salt), &ikm)
            .expand(&info, &mut okm)
            .expect("32 bytes is a valid HKDF-SHA256 output length");
        let cipher = ChaCha20Poly1305::new(&Key::from(okm));
        okm.fill(0);
        cipher
    }

    /// The token from the bytes after the magic; None for a wrong key, tampering,
    /// truncation or a missing salt.
    fn open_sealed(&self, sealed: &[u8]) -> Option<String> {
        if sealed.len() < NONCE_LEN + TAG_LEN {
            return None;
        }
        let salt = self.read_salt()?;
        let (nonce, ciphertext) = sealed.split_at(NONCE_LEN);
        let nonce = Nonce::try_from(nonce).ok()?;
        let payload = Payload {
            msg: ciphertext,
            aad: self.account.as_bytes(),
        };
        let plain = self.cipher(&salt).decrypt(&nonce, payload).ok()?;
        String::from_utf8(plain).ok().filter(|t| !t.is_empty())
    }

    fn write_file(&self, token: &str) -> io::Result<()> {
        log::info!("storing the sign-in token in {}", self.dir.display());
        let salt = self.salt_for_write()?;
        let mut nonce = [0u8; NONCE_LEN];
        random_bytes(&mut nonce)?;
        let payload = Payload {
            msg: token.as_bytes(),
            aad: self.account.as_bytes(),
        };
        let ciphertext = self
            .cipher(&salt)
            .encrypt(&Nonce::from(nonce), payload)
            .map_err(|_| io::Error::other("could not encrypt the token"))?;
        let mut out = Vec::with_capacity(MAGIC.len() + NONCE_LEN + ciphertext.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ciphertext);
        let path = self.file();
        let tmp = self.write_tmp(&path, &out)?;
        fs::rename(&tmp, &path)
    }

    fn read_salt(&self) -> Option<[u8; SALT_LEN]> {
        fs::read(self.salt_file()).ok()?.try_into().ok()
    }

    /// The salt, made (once per config dir) when there is none.
    fn salt_for_write(&self) -> io::Result<[u8; SALT_LEN]> {
        if let Some(salt) = self.read_salt() {
            return Ok(salt);
        }
        let path = self.salt_file();
        // A salt file of the wrong size is useless: nothing sealed with it can be read.
        let _ = fs::remove_file(&path);
        let mut salt = [0u8; SALT_LEN];
        random_bytes(&mut salt)?;
        let tmp = self.write_tmp(&path, &salt)?;
        // Link rather than rename so a salt another process has just made is never replaced.
        match fs::hard_link(&tmp, &path) {
            Ok(()) => {
                let _ = fs::remove_file(&tmp);
                Ok(salt)
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                let _ = fs::remove_file(&tmp);
                self.read_salt()
                    .ok_or_else(|| io::Error::other("the token salt file is unreadable"))
            }
            // A file system without hard links.
            Err(_) => fs::rename(&tmp, &path).map(|()| salt),
        }
    }

    /// Writes `bytes` to a private temp file next to `path` (0600 from the first byte on
    /// Unix, in a 0700 dir) and returns its path for the caller to move into place.
    fn write_tmp(&self, path: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
        use std::io::Write;
        let tmp = path.with_extension("tmp");
        let _ = fs::remove_file(&tmp);
        #[cfg(unix)]
        let mut f = {
            use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&self.dir)?;
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)?
        };
        #[cfg(not(unix))]
        let mut f = {
            fs::create_dir_all(&self.dir)?;
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)?
        };
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok(tmp)
    }

    /// Removes the token (keychain entry and file). The salt stays.
    pub fn clear(&self) {
        if let Some(e) = self.entry() {
            match e.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(err) => log::debug!("keychain delete failed: {err}"),
            }
        }
        let _ = fs::remove_file(self.file());
    }
}

/// Call with any error from an API call: a dead token (401) is cleared so the app goes to
/// sign-in. Returns true when it cleared the token.
pub fn handle_unauthorized(store: &TokenStore, err: &AccountError) -> bool {
    if matches!(err, AccountError::Unauthorized) {
        store.clear();
        true
    } else {
        false
    }
}

fn random_bytes(buf: &mut [u8]) -> io::Result<()> {
    getrandom::fill(buf).map_err(|e| io::Error::other(format!("no random bytes from the OS: {e}")))
}

/// A token in the old plaintext file format: one line of printable ASCII.
fn legacy_token(bytes: &[u8]) -> Option<String> {
    let t = std::str::from_utf8(bytes).ok()?.trim();
    let looks_like_a_token =
        !t.is_empty() && t.len() <= 8192 && t.bytes().all(|b| b.is_ascii_graphic());
    looks_like_a_token.then(|| t.to_string())
}

/// machine id ‖ user id ‖ user name, each length-prefixed. With no readable machine id the
/// home dir path stands in (weaker: any machine with the same path and user gives the same key).
fn machine_key_material() -> Vec<u8> {
    let home = directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf());
    let machine = machine_id().unwrap_or_else(|| {
        log::warn!(
            "no machine id found; the encrypted token file is bound only to this user account (weaker)"
        );
        let home = home.as_deref().map(|h| h.to_string_lossy().into_owned());
        format!("home:{}", home.unwrap_or_default())
    });
    let user = user_id(home.as_deref());
    let name = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default();
    let mut out = Vec::new();
    for part in [machine.as_bytes(), user.as_bytes(), name.as_bytes()] {
        out.extend_from_slice(&(part.len() as u32).to_le_bytes());
        out.extend_from_slice(part);
    }
    out
}

/// The owner uid of the home dir (no libc dependency needed).
#[cfg(unix)]
fn user_id(home: Option<&Path>) -> String {
    use std::os::unix::fs::MetadataExt;
    match home.and_then(|h| fs::metadata(h).ok()) {
        Some(m) => format!("uid:{}", m.uid()),
        None => String::new(),
    }
}

#[cfg(not(unix))]
fn user_id(home: Option<&Path>) -> String {
    home.map(|h| format!("home:{}", h.to_string_lossy()))
        .unwrap_or_default()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn machine_id() -> Option<String> {
    ["/etc/machine-id", "/var/lib/dbus/machine-id"]
        .iter()
        .find_map(|p| {
            let id = fs::read_to_string(p).ok()?;
            let id = id.trim();
            (!id.is_empty()).then(|| id.to_string())
        })
}

#[cfg(target_os = "macos")]
fn machine_id() -> Option<String> {
    let out = std::process::Command::new("ioreg")
        .args(["-rd1", "-c", "IOPlatformExpertDevice"])
        .output()
        .ok()?;
    parse_ioreg_uuid(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(windows)]
fn machine_id() -> Option<String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new("reg")
        .args([
            "query",
            r"HKLM\SOFTWARE\Microsoft\Cryptography",
            "/v",
            "MachineGuid",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    parse_reg_machine_guid(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(not(any(unix, windows)))]
fn machine_id() -> Option<String> {
    None
}

/// The value of `"IOPlatformUUID" = "…"` in `ioreg -rd1 -c IOPlatformExpertDevice` output.
#[cfg(any(target_os = "macos", test))]
fn parse_ioreg_uuid(out: &str) -> Option<String> {
    let line = out.lines().find(|l| l.contains("\"IOPlatformUUID\""))?;
    let value = line.split_once('=')?.1.trim().trim_matches('"').trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// The value from `reg query …\Cryptography /v MachineGuid` (`MachineGuid  REG_SZ  <guid>`).
#[cfg(any(windows, test))]
fn parse_reg_machine_guid(out: &str) -> Option<String> {
    let line = out.lines().find(|l| l.contains("MachineGuid"))?;
    let mut words = line.split_whitespace();
    words.find(|w| *w == "REG_SZ")?;
    words.next().map(str::to_string)
}

/// The project config dir, or (when the platform has none, e.g. no home directory) a folder
/// in the temp dir with a warning; never the current directory.
fn config_dir(project: Option<PathBuf>) -> PathBuf {
    project.unwrap_or_else(|| {
        let dir = std::env::temp_dir().join("lensflow-ImageEditor");
        log::warn!(
            "no config folder for this user; the token file (if needed) goes in {}",
            dir.display()
        );
        dir
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_project_dir_falls_back_to_the_temp_dir_not_the_current_dir() {
        let dir = config_dir(None);
        assert_eq!(dir, std::env::temp_dir().join("lensflow-ImageEditor"));
        assert!(dir.is_absolute());
        let given = PathBuf::from("/x/config");
        assert_eq!(config_dir(Some(given.clone())), given);
    }

    #[test]
    fn parses_the_macos_platform_uuid() {
        let out = concat!(
            "+-o J314sAP  <class IOPlatformExpertDevice>\n",
            "    {\n",
            "      \"IOPlatformSerialNumber\" = \"C02XX\"\n",
            "      \"IOPlatformUUID\" = \"1A2B3C4D-0000-1111-2222-333344445555\"\n",
            "    }\n",
        );
        assert_eq!(
            parse_ioreg_uuid(out).as_deref(),
            Some("1A2B3C4D-0000-1111-2222-333344445555")
        );
        assert_eq!(parse_ioreg_uuid("nothing here"), None);
    }

    #[test]
    fn parses_the_windows_machine_guid() {
        let out = concat!(
            "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Cryptography\r\n",
            "    MachineGuid    REG_SZ    0f1e2d3c-aaaa-bbbb-cccc-0123456789ab\r\n\r\n",
        );
        assert_eq!(
            parse_reg_machine_guid(out).as_deref(),
            Some("0f1e2d3c-aaaa-bbbb-cccc-0123456789ab")
        );
        assert_eq!(parse_reg_machine_guid(""), None);
    }

    #[test]
    fn legacy_tokens_are_one_printable_line() {
        assert_eq!(legacy_token(b"12|abc\n").as_deref(), Some("12|abc"));
        assert_eq!(legacy_token(b""), None);
        assert_eq!(legacy_token(b"two words"), None);
        assert_eq!(legacy_token(&[0xff, 0x00, 0x13]), None);
    }

    #[test]
    fn machine_key_material_is_stable_and_not_empty() {
        let a = MachineKey.key_material();
        assert!(a.len() > 12);
        assert_eq!(a, MachineKey.key_material());
    }
}
