//! The account side of the app: the stored credentials, and the jobs the `account` lane
//! runs (resume, sign in, me, the entitlement gate, sign-out and clearing). Every keychain
//! and cache access happens in these jobs, never on the UI thread, and the account lane
//! runs them in order, so a clear always lands after any check already queued.
//!
//! Each job carries the [`Generation`] it was started in and its answer echoes it; the app
//! drops answers from an older generation (before a studio switch, a sign-out, a 401 or a
//! server switch). Nothing here logs the token or the password.

use crate::jobs::Msg;
use lf_account::auth::{self, SignedIn};
use lf_account::entitlement::{Cache, Decision, Gate};
use lf_account::{AccountError, Client, Instance, TokenStore};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Which round of account work an answer belongs to.
pub type Generation = u64;

/// The sign-in token on its way from the keychain to the UI; it never prints.
pub struct Token(pub String);

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

/// The sign-in token and the cached entitlement, the two things a sign-out forgets.
pub struct Credentials {
    tokens: TokenStore,
    cache_dir: PathBuf,
    /// The entitlement cache's file name, one per server ([`cache_file`]).
    cache_file: String,
}

/// The entitlement cache's file name for `instance`: `entitlement.json` for LensFlow,
/// `entitlement-dev.json` for the development server (and `entitlement-<host>.json` for any
/// other), so switching servers never replays one server's answer for the other.
pub fn cache_file(instance: &Instance) -> String {
    match instance {
        Instance::Prod => "entitlement.json".to_owned(),
        Instance::Dev => "entitlement-dev.json".to_owned(),
        Instance::Custom(url) => {
            let host: String = url
                .url()
                .host_str()
                .unwrap_or("custom")
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect();
            format!("entitlement-{host}.json")
        }
    }
}

impl Credentials {
    /// The OS keychain (or its file fallback) for `instance`; the entitlement cache in
    /// `config_dir`, one file per server.
    pub fn open(instance: &Instance, config_dir: &Path) -> Credentials {
        Credentials {
            tokens: TokenStore::open(instance),
            cache_dir: config_dir.to_path_buf(),
            cache_file: cache_file(instance),
        }
    }

    /// Everything in `dir`, no keychain (tests).
    pub fn in_dir(instance: &Instance, dir: &Path) -> Credentials {
        Credentials {
            tokens: TokenStore::open_in(instance.base_url().as_str(), dir, false),
            cache_dir: dir.to_path_buf(),
            cache_file: cache_file(instance),
        }
    }

    pub fn token(&self) -> Option<String> {
        self.tokens.load()
    }

    pub fn save_token(&self, token: &str) {
        self.tokens.save(token);
    }

    /// Where `Gate::refresh` keeps the last good entitlement (this server's file).
    pub fn cache(&self) -> Cache {
        Cache::named(&self.cache_dir, &self.cache_file)
    }

    /// Drops the token and the cached entitlement.
    pub fn forget(&self) {
        self.tokens.clear();
        self.forget_entitlement();
    }

    /// The cached entitlement alone (switching studio: never replay another studio's).
    pub fn forget_entitlement(&self) {
        self.cache().clear();
    }
}

/// A name for this computer on the studio's list of signed-in devices.
fn device_name() -> String {
    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| std::env::consts::OS.to_owned());
    format!("lensflow-ImageEditor on {host}")
}

/// At launch: the stored token, if any.
pub fn resume(generation: Generation, creds: &Credentials) -> Msg {
    Msg::Resumed(generation, creds.token().map(Token))
}

/// `auth/login`; a token that comes back is stored before the answer goes to the UI.
pub fn sign_in(
    generation: Generation,
    client: &Client,
    creds: &Credentials,
    email: &str,
    password: &str,
    code: Option<&str>,
) -> Msg {
    let result: Result<SignedIn, AccountError> =
        auth::sign_in(client, email, password, code, &device_name());
    if let Ok(signed) = &result {
        creds.save_token(&signed.token);
    }
    Msg::SignedIn(generation, result)
}

/// `auth/me`; a 401 forgets the credentials.
pub fn me(generation: Generation, client: &Client, creds: &Credentials) -> Msg {
    let result = auth::me(client);
    if let Err(AccountError::Unauthorized) = &result {
        creds.forget();
    }
    Msg::Me(generation, result)
}

/// `desktop/entitlement` through the gate for the studio `studio_id` (`client` carries the
/// token and that studio). A `SignIn` decision forgets the token too (the gate has already
/// cleared the cache).
pub fn gate(generation: Generation, client: &Client, creds: &Credentials, studio_id: u64) -> Msg {
    let decision = Gate::refresh(client, &creds.cache(), studio_id, SystemTime::now());
    if matches!(decision, Decision::SignIn) {
        creds.forget();
    }
    Msg::Gate(generation, decision)
}

/// Sign-out: tells the server (when there is a token; the answer does not matter) and
/// forgets the credentials.
pub fn sign_out(client: Option<&Client>, creds: &Credentials) -> Msg {
    if let Some(client) = client {
        auth::sign_out(client);
    }
    creds.forget();
    Msg::Cleared
}

/// After a 401 from any other call: forgets the credentials.
pub fn forget(creds: &Credentials) -> Msg {
    creds.forget();
    Msg::Cleared
}

/// Switching studio: forgets the cached entitlement only.
pub fn forget_entitlement(creds: &Credentials) -> Msg {
    creds.forget_entitlement();
    Msg::Cleared
}

/// The error was a hiccup (no connection, rate limit, server trouble, a garbled reply),
/// not an answer.
pub fn is_transient(err: &AccountError) -> bool {
    matches!(
        err,
        AccountError::Network(_)
            | AccountError::RateLimited
            | AccountError::BadReply(_)
            | AccountError::Server(500..=599, _)
    )
}

/// What the sign-in form shows under the fields for a failed `auth/login`.
pub fn sign_in_error(err: &AccountError) -> String {
    match err {
        AccountError::Validation(fields) => fields.values().cloned().collect::<Vec<_>>().join(" "),
        AccountError::Network(_) => format!("Could not reach LensFlow. {err}"),
        AccountError::RateLimited => "Too many attempts. Wait a minute and try again.".to_owned(),
        AccountError::Unauthorized => "The email or password is not right.".to_owned(),
        other => format!("Could not sign in. {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{Jobs, Lane};
    use eframe::egui;
    use lf_account::entitlement::{Entitlement, Standing};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn a_clear_queued_after_a_running_check_lands_after_it() {
        let dir = tempfile::tempdir().unwrap();
        let creds = Arc::new(Credentials::in_dir(&Instance::Dev, dir.path()));
        let server = httpmock::MockServer::start();
        let body = serde_json::json!({
            "data": serde_json::to_value(Entitlement::example(Standing::Active, true)).unwrap()
        });
        server.mock(|when, then| {
            when.path("/api/desktop/entitlement");
            then.status(200)
                .delay(Duration::from_millis(300))
                .json_body(body);
        });
        let url = reqwest::Url::parse(&server.url("/api")).unwrap();
        let client = Client::new(Instance::custom(url).unwrap(), "1.0.0")
            .with_token("t")
            .with_studio("frames");

        // A check for studio A is running when the user switches studio.
        let jobs = Jobs::new(egui::Context::default());
        let gate_creds = Arc::clone(&creds);
        jobs.spawn(Lane::Account, "entitlement", move || {
            gate(1, &client, &gate_creds, 1)
        });
        let clear_creds = Arc::clone(&creds);
        jobs.spawn(Lane::Account, "forget entitlement", move || {
            forget_entitlement(&clear_creds)
        });

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut got = Vec::new();
        while got.len() < 2 && Instant::now() < deadline {
            got.extend(jobs.drain());
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            matches!(
                got.as_slice(),
                [Msg::Gate(1, Decision::Allow(_)), Msg::Cleared]
            ),
            "{got:?}"
        );
        assert!(
            creds.cache().load().is_none(),
            "the check's save was cleared after it"
        );
    }

    #[test]
    fn the_resumed_token_never_prints() {
        let dir = tempfile::tempdir().unwrap();
        let creds = Credentials::in_dir(&Instance::Dev, dir.path());
        creds.save_token("secret-token");
        let msg = resume(3, &creds);
        assert!(matches!(&msg, Msg::Resumed(3, Some(Token(t))) if t == "secret-token"));
        assert!(!format!("{msg:?}").contains("secret-token"));
    }

    #[test]
    fn each_server_keeps_its_own_entitlement_cache() {
        let dir = tempfile::tempdir().unwrap();
        let prod = Credentials::in_dir(&Instance::Prod, dir.path());
        let dev = Credentials::in_dir(&Instance::Dev, dir.path());
        dev.cache()
            .save(&Entitlement::example(Standing::Active, true));
        assert!(dev.cache().load().is_some());
        assert!(prod.cache().load().is_none(), "never replayed for LensFlow");
        assert!(dir.path().join("entitlement-dev.json").exists());
        prod.cache()
            .save(&Entitlement::example(Standing::Active, true));
        assert!(dir.path().join("entitlement.json").exists());
        dev.forget_entitlement();
        assert!(dev.cache().load().is_none());
        assert!(
            prod.cache().load().is_some(),
            "forgetting one leaves the other"
        );
    }
}
