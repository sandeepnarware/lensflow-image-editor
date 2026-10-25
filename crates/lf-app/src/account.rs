//! The account side of the app: the stored credentials, and the calls the `account` lane
//! runs (sign in, me, the entitlement gate). Nothing here logs the token or the password.

use crate::jobs::Msg;
use crate::session::Session;
use lf_account::auth::{self, StudioSummary, User};
use lf_account::entitlement::{Cache, Decision, Gate};
use lf_account::{AccountError, Client, Instance, TokenStore};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The sign-in token and the cached entitlement, the two things a sign-out forgets.
pub struct Credentials {
    tokens: TokenStore,
    cache_dir: PathBuf,
}

impl Credentials {
    /// The OS keychain (or its file fallback) for `instance`; the entitlement cache in
    /// `config_dir`.
    pub fn open(instance: &Instance, config_dir: &Path) -> Credentials {
        Credentials {
            tokens: TokenStore::open(instance),
            cache_dir: config_dir.to_path_buf(),
        }
    }

    /// Everything in `dir`, no keychain (tests).
    pub fn in_dir(instance: &Instance, dir: &Path) -> Credentials {
        Credentials {
            tokens: TokenStore::open_in(instance.base_url().as_str(), dir, false),
            cache_dir: dir.to_path_buf(),
        }
    }

    pub fn token(&self) -> Option<String> {
        self.tokens.load()
    }

    pub fn save_token(&self, token: &str) {
        self.tokens.save(token);
    }

    /// Where `Gate::refresh` keeps the last good entitlement.
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Drops the token and the cached entitlement.
    pub fn forget(&self) {
        self.tokens.clear();
        self.forget_entitlement();
    }

    /// The cached entitlement alone (switching studio: never replay another studio's).
    pub fn forget_entitlement(&self) {
        Cache::new(&self.cache_dir).clear();
    }

    /// Applies a gate decision; a `SignIn` decision also forgets the credentials.
    pub fn settle(
        &self,
        session: Session,
        decision: Decision,
        user: &User,
        studio: &StudioSummary,
        current_version: &str,
    ) -> Session {
        if matches!(decision, Decision::SignIn) {
            self.forget();
        }
        session.on_decision(decision, user, studio, current_version)
    }

    /// For any error from an account call: a 401 clears the token and the cache, moves
    /// `session` to sign-in and returns true; anything else leaves both alone.
    pub fn signed_out_by(&self, session: &mut Session, err: &AccountError) -> bool {
        if !lf_account::handle_unauthorized(&self.tokens, err) {
            return false;
        }
        self.forget_entitlement();
        let current = std::mem::replace(session, Session::signing_in());
        *session = current.on_unauthorized();
        true
    }
}

/// A name for this computer on the studio's list of signed-in devices.
fn device_name() -> String {
    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| std::env::consts::OS.to_owned());
    format!("lensflow-ImageEditor on {host}")
}

/// `auth/login` (the account lane).
pub fn sign_in(client: &Client, email: &str, password: &str, code: Option<&str>) -> Msg {
    Msg::SignedIn(auth::sign_in(client, email, password, code, &device_name()))
}

/// `auth/me` (the account lane).
pub fn me(client: &Client) -> Msg {
    Msg::Me(auth::me(client))
}

/// `desktop/entitlement` through the gate (the account lane); `client` carries the token
/// and the studio.
pub fn gate(client: &Client, cache_dir: &Path) -> Msg {
    Msg::Gate(Gate::refresh(
        client,
        &Cache::new(cache_dir),
        SystemTime::now(),
    ))
}

/// The error was a hiccup (no connection, rate limit, server trouble), not an answer.
pub fn is_transient(err: &AccountError) -> bool {
    matches!(
        err,
        AccountError::Network(_) | AccountError::RateLimited | AccountError::Server(500..=599, _)
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
