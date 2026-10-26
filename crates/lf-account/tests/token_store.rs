use lf_account::TokenStore;

#[test]
fn file_fallback_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::open_in("https://lensflow.test/api", dir.path(), false);
    assert_eq!(store.load(), None);
    store.save("secret-token");
    assert_eq!(store.load().as_deref(), Some("secret-token"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let file = std::fs::read_dir(dir.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(
            std::fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    // A different base URL has its own token.
    let other = TokenStore::open_in("https://dev.lensflow.test/api", dir.path(), false);
    assert_eq!(other.load(), None);
    store.clear();
    assert_eq!(store.load(), None);
    store.clear(); // clearing twice is fine
}

#[test]
fn keyring_round_trip_when_the_backend_exists() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("https://keyring-test-{}.test/api", std::process::id());
    let store = TokenStore::open_in(&url, dir.path(), true);
    if !store.keyring_works() {
        eprintln!("no keychain here, skipping the keyring round trip");
        return;
    }
    store.save("kr-token");
    assert_eq!(store.load().as_deref(), Some("kr-token"));
    assert!(
        std::fs::read_dir(dir.path()).unwrap().next().is_none(),
        "no file when the keychain took it"
    );
    store.clear();
    assert_eq!(store.load(), None);
}

#[test]
fn a_file_token_is_found_when_the_keyring_is_enabled_but_empty() {
    // The path a failed keychain write takes: the file holds it, load finds it.
    let dir = tempfile::tempdir().unwrap();
    let url = format!("https://fallback-{}.test/api", std::process::id());
    let file_only = TokenStore::open_in(&url, dir.path(), false);
    file_only.save("t");
    let both = TokenStore::open_in(&url, dir.path(), true);
    assert_eq!(both.load().as_deref(), Some("t"));
    both.clear();
    assert_eq!(file_only.load(), None);
}

#[test]
fn keyring_probe_does_not_destroy_a_stored_token() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("https://probe-test-{}.test/api", std::process::id());
    let store = TokenStore::open_in(&url, dir.path(), true);
    if !store.keyring_works() {
        eprintln!("no keychain here, skipping");
        return;
    }
    store.save("keep-me");
    assert!(store.keyring_works());
    assert_eq!(store.load().as_deref(), Some("keep-me"));
    store.clear();
}

#[cfg(unix)]
#[test]
fn config_dir_is_private_and_file_is_never_world_readable() {
    use std::os::unix::fs::PermissionsExt;
    let base = tempfile::tempdir().unwrap();
    let dir = base.path().join("cfg");
    let store = TokenStore::open_in("https://perm.test/api", &dir, false);
    store.save("t");
    assert_eq!(
        std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    store.save("t2");
    for e in std::fs::read_dir(&dir).unwrap() {
        let m = e.unwrap().metadata().unwrap().permissions().mode() & 0o777;
        assert_eq!(m, 0o600);
    }
}

// --- The encrypted file fallback ---------------------------------------------------------

use lf_account::token_store::KeySource;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Simulates a machine and user account: the bytes stand for machine id, user id and name.
struct Machine(&'static str);

impl KeySource for Machine {
    fn key_material(&self) -> Vec<u8> {
        self.0.as_bytes().to_vec()
    }
}

fn store_on(machine: &'static str, url: &str, dir: &Path) -> TokenStore {
    TokenStore::open_with_key(url, dir, false, Arc::new(Machine(machine)))
}

/// The token file for a store (every `token-*` file except the shared salt).
fn token_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            let n = p.file_name().unwrap().to_string_lossy().into_owned();
            n.starts_with("token-") && n != "token-salt"
        })
        .collect();
    v.sort();
    v
}

fn only_token_file(dir: &Path) -> PathBuf {
    let files = token_files(dir);
    assert_eq!(files.len(), 1, "expected one token file, got {files:?}");
    files.into_iter().next().unwrap()
}

const TOKEN: &str = "42|plain-sanctum-token-ABCDEFGHIJKLMNOP";

#[test]
fn encrypted_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_on("machine-a", "https://enc.test/api", dir.path());
    assert_eq!(store.load(), None);
    store.save(TOKEN);
    assert_eq!(store.load().as_deref(), Some(TOKEN));
    // A fresh store on the same machine reads it too.
    let again = store_on("machine-a", "https://enc.test/api", dir.path());
    assert_eq!(again.load().as_deref(), Some(TOKEN));
}

#[test]
fn the_file_is_encrypted_not_plaintext() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_on("machine-a", "https://enc.test/api", dir.path());
    store.save(TOKEN);
    let bytes = std::fs::read(only_token_file(dir.path())).unwrap();
    assert!(
        bytes.starts_with(b"LFT1"),
        "the file starts with the LFT1 magic"
    );
    let needle = TOKEN.as_bytes();
    assert!(
        !bytes.windows(needle.len()).any(|w| w == needle),
        "the token is not in the file"
    );
    // Nor any long piece of it.
    let piece = &needle[3..15];
    assert!(!bytes.windows(piece.len()).any(|w| w == piece));
    assert!(
        dir.path().join("token-salt").exists(),
        "the salt file exists"
    );
}

#[test]
fn another_machine_cannot_read_the_file_and_it_is_removed() {
    let dir = tempfile::tempdir().unwrap();
    let url = "https://enc.test/api";
    store_on("machine-a", url, dir.path()).save(TOKEN);
    let file = only_token_file(dir.path());
    let other = store_on("machine-b", url, dir.path());
    assert_eq!(other.load(), None);
    assert!(!file.exists(), "an unreadable token file is deleted");
    // The salt stays.
    assert!(dir.path().join("token-salt").exists());
}

#[test]
fn a_tampered_file_returns_none() {
    let dir = tempfile::tempdir().unwrap();
    let url = "https://enc.test/api";
    let store = store_on("machine-a", url, dir.path());
    store.save(TOKEN);
    let file = only_token_file(dir.path());
    let mut bytes = std::fs::read(&file).unwrap();
    let last = bytes.len() - 20; // inside the ciphertext, past the magic and nonce
    bytes[last] ^= 0x01;
    std::fs::write(&file, &bytes).unwrap();
    assert_eq!(store.load(), None);
    assert!(!file.exists());
}

#[test]
fn a_truncated_file_returns_none() {
    let dir = tempfile::tempdir().unwrap();
    let url = "https://enc.test/api";
    let store = store_on("machine-a", url, dir.path());
    store.save(TOKEN);
    let file = only_token_file(dir.path());
    let bytes = std::fs::read(&file).unwrap();
    std::fs::write(&file, &bytes[..10]).unwrap();
    assert_eq!(store.load(), None);
    assert!(!file.exists());
}

#[test]
fn a_legacy_plaintext_file_is_migrated() {
    let dir = tempfile::tempdir().unwrap();
    let url = "https://legacy.test/api";
    let store = store_on("machine-a", url, dir.path());
    // Find the file name by saving once, then put the old plaintext format in its place.
    store.save("x");
    let file = only_token_file(dir.path());
    std::fs::write(&file, format!("{TOKEN}\n")).unwrap();

    assert_eq!(store.load().as_deref(), Some(TOKEN));
    let bytes = std::fs::read(&file).unwrap();
    assert!(bytes.starts_with(b"LFT1"), "re-saved encrypted");
    assert!(!bytes.windows(TOKEN.len()).any(|w| w == TOKEN.as_bytes()));
    assert_eq!(store.load().as_deref(), Some(TOKEN));
}

#[test]
fn a_legacy_file_without_a_salt_is_migrated() {
    // Upgrading from the old build: only the plaintext file, no salt yet.
    let dir = tempfile::tempdir().unwrap();
    let url = "https://legacy2.test/api";
    let store = store_on("machine-a", url, dir.path());
    store.save("x");
    let file = only_token_file(dir.path());
    std::fs::remove_file(dir.path().join("token-salt")).unwrap();
    std::fs::write(&file, TOKEN).unwrap();
    assert_eq!(store.load().as_deref(), Some(TOKEN));
    assert!(std::fs::read(&file).unwrap().starts_with(b"LFT1"));
    assert!(dir.path().join("token-salt").exists());
}

#[test]
fn two_base_urls_do_not_read_each_others_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let a = store_on("machine-a", "https://a.test/api", dir.path());
    let b = store_on("machine-a", "https://b.test/api", dir.path());
    a.save("token-for-a");
    b.save("token-for-b");
    assert_eq!(a.load().as_deref(), Some("token-for-a"));
    assert_eq!(b.load().as_deref(), Some("token-for-b"));
    // Copying a's file over b's does not hand b's store a's token (the URL is bound in).
    let files = token_files(dir.path());
    assert_eq!(files.len(), 2);
    let a_bytes = std::fs::read(&files[0]).unwrap();
    let b_bytes = std::fs::read(&files[1]).unwrap();
    std::fs::write(&files[1], &a_bytes).unwrap();
    std::fs::write(&files[0], &b_bytes).unwrap();
    assert_eq!(a.load(), None);
    assert_eq!(b.load(), None);
}

#[test]
fn clear_removes_the_token_file_and_keeps_the_salt() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_on("machine-a", "https://enc.test/api", dir.path());
    store.save(TOKEN);
    let salt = std::fs::read(dir.path().join("token-salt")).unwrap();
    assert_eq!(salt.len(), 16);
    store.clear();
    assert!(token_files(dir.path()).is_empty());
    assert_eq!(std::fs::read(dir.path().join("token-salt")).unwrap(), salt);
    // A new save reuses the salt.
    store.save(TOKEN);
    assert_eq!(std::fs::read(dir.path().join("token-salt")).unwrap(), salt);
    assert_eq!(store.load().as_deref(), Some(TOKEN));
}

#[cfg(unix)]
#[test]
fn token_and_salt_files_are_0600() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let store = store_on("machine-a", "https://enc.test/api", dir.path());
    store.save(TOKEN);
    for p in [only_token_file(dir.path()), dir.path().join("token-salt")] {
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600,
            "{p:?}"
        );
    }
}
