use crate::{AccountError, Instance};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

const SERVICE: &str = "lensflow-ImageEditor";

/// The sign-in token: OS keychain first, a 0600 file under the config dir when there is none.
pub struct TokenStore {
    account: String,
    dir: PathBuf,
    use_keyring: bool,
}

impl TokenStore {
    pub fn open(instance: &Instance) -> TokenStore {
        let dir = directories::ProjectDirs::from("in", "lensflow", "lensflow-ImageEditor")
            .map(|d| d.config_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        Self::open_in(instance.base_url().as_str(), &dir, true)
    }

    /// For tests and the app's own config dir: `use_keyring = false` keeps everything in the file.
    pub fn open_in(base_url: &str, dir: &Path, use_keyring: bool) -> TokenStore {
        TokenStore {
            account: base_url.to_string(),
            dir: dir.to_path_buf(),
            use_keyring,
        }
    }

    fn entry(&self) -> Option<keyring::Entry> {
        if !self.use_keyring {
            return None;
        }
        keyring::Entry::new(SERVICE, &self.account).ok()
    }

    fn file(&self) -> PathBuf {
        let hash = hex::encode(Sha256::digest(self.account.as_bytes()));
        self.dir.join(format!("token-{}", &hash[..16]))
    }

    /// True when the keychain accepts a write and a read back (tests use it to skip cleanly).
    pub fn keyring_works(&self) -> bool {
        let Some(e) = self.entry() else { return false };
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
        let t = fs::read_to_string(self.file()).ok()?;
        let t = t.trim().to_string();
        (!t.is_empty()).then_some(t)
    }

    pub fn save(&self, token: &str) {
        if let Some(e) = self.entry() {
            match e.set_password(token) {
                Ok(()) => {
                    let _ = fs::remove_file(self.file());
                    return;
                }
                Err(err) => {
                    log::warn!("no usable keychain ({err}); keeping the token in a private file")
                }
            }
        }
        if let Err(err) = self.write_file(token) {
            log::error!("could not store the sign-in token: {err}");
        }
    }

    fn write_file(&self, token: &str) -> std::io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let path = self.file();
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&path)?;
            f.write_all(token.as_bytes())?;
            // open() keeps the mode of an existing file: force it.
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        }
        #[cfg(not(unix))]
        fs::write(&path, token)
    }

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
