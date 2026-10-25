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
        let project = directories::ProjectDirs::from("in", "lensflow", "lensflow-ImageEditor")
            .map(|d| d.config_dir().to_path_buf());
        Self::open_in(instance.base_url().as_str(), &config_dir(project), true)
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
                        "the OS keychain is not available ({err}); the sign-in token is kept in a private file"
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
        let t = fs::read_to_string(self.file()).ok()?;
        let t = t.trim().to_string();
        if t.is_empty() {
            return None;
        }
        log::info!("using the sign-in token file (no keychain entry)");
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
                    log::warn!("no usable keychain ({err}); keeping the token in a private file")
                }
            }
        }
        if let Err(err) = self.write_file(token) {
            log::error!("could not store the sign-in token: {err}");
        }
    }

    fn write_file(&self, token: &str) -> std::io::Result<()> {
        log::info!("storing the sign-in token in {}", self.dir.display());
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&self.dir)?;
            let path = self.file();
            // Private from the first byte: a 0600 temp file, renamed into place.
            let tmp = path.with_extension("tmp");
            let _ = fs::remove_file(&tmp);
            let mut f = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)?;
            f.write_all(token.as_bytes())?;
            f.sync_all()?;
            fs::rename(&tmp, &path)
        }
        #[cfg(not(unix))]
        {
            fs::create_dir_all(&self.dir)?;
            fs::write(self.file(), token)
        }
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
}
