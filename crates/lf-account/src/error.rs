use std::collections::BTreeMap;

/// Why the app is locked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockReason {
    Suspended,
    Paused,
    NotEnabled,
    /// No fresh check and no usable cached one.
    Offline,
}

#[derive(Debug, thiserror::Error)]
pub enum AccountError {
    #[error("network error: {0}")]
    Network(String),
    #[error("signed out: the token is no longer valid")]
    Unauthorized,
    #[error("{1}")]
    Locked(LockReason, String),
    #[error("{0}")]
    ReadOnly(String),
    #[error("{0}")]
    PlanLimit(String),
    #[error("too many requests, slow down")]
    RateLimited,
    #[error("server error {0}: {1}")]
    Server(u16, String),
    #[error("a two-factor code is required")]
    TwoFactorRequired,
    #[error("validation failed: {0:?}")]
    Validation(BTreeMap<String, String>),
}
