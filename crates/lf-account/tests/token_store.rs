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
