use crate::{AccountError, Client, LockReason};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A cached entitlement is trusted for this long after `checked_at`.
pub const GRACE: Duration = Duration::from_secs(7 * 86_400);
/// A cache dated further ahead than this is treated as expired (a clock set back).
const CLOCK_SKEW: Duration = Duration::from_secs(300);
const OFFLINE_MESSAGE: &str = "Connect to the internet to keep using lensflow-ImageEditor.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Standing {
    Active,
    ReadOnly,
    Paused,
    Suspended,
    /// A standing this version does not know (a newer server): treated as read-only, so a
    /// payload change never locks the app.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiStatus {
    pub available: bool,
    #[serde(default)]
    pub remaining: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StudioInfo {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub logo_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub published_at: Option<String>,
}

mod secs {
    use super::*;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer>(t: &SystemTime, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(
            t.duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        )
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SystemTime, D::Error> {
        Ok(UNIX_EPOCH + Duration::from_secs(u64::deserialize(d)?))
    }

    pub fn epoch() -> SystemTime {
        UNIX_EPOCH
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entitlement {
    pub enabled: bool,
    pub standing: Standing,
    pub ai: AiStatus,
    pub studio: StudioInfo,
    #[serde(default)]
    pub min_version: Option<String>,
    #[serde(default)]
    pub latest: Option<Release>,
    #[serde(with = "secs", default = "secs::epoch")]
    pub checked_at: SystemTime,
}

impl Entitlement {
    /// A plausible entitlement for tests and previews.
    #[doc(hidden)]
    pub fn example(standing: Standing, enabled: bool) -> Entitlement {
        Entitlement {
            enabled,
            standing,
            ai: AiStatus {
                available: true,
                remaining: Some(10),
            },
            studio: StudioInfo {
                id: 1,
                name: "Studio".into(),
                logo_url: None,
            },
            min_version: None,
            latest: None,
            checked_at: SystemTime::now(),
        }
    }
}

/// `GET desktop/entitlement`.
pub fn check(client: &Client) -> Result<Entitlement, AccountError> {
    let reply = client.get_json("desktop/entitlement")?;
    let data = reply.get("data").cloned().unwrap_or(reply);
    let mut e: Entitlement = serde_json::from_value(data).map_err(|e| {
        log::warn!("the entitlement reply could not be read: {e}");
        AccountError::BadReply(format!("unexpected entitlement reply: {e}"))
    })?;
    e.checked_at = SystemTime::now();
    Ok(e)
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DownloadLink {
    pub url: String,
    pub sha256: String,
    pub version: String,
    pub size: u64,
    pub expires_at: String,
}

/// `GET desktop/download/{platform}`.
pub fn download_link(client: &Client, platform: &str) -> Result<DownloadLink, AccountError> {
    let reply = client.get_json(&format!("desktop/download/{platform}"))?;
    let data: Value = reply.get("data").cloned().unwrap_or(reply);
    serde_json::from_value(data)
        .map_err(|e| AccountError::Server(200, format!("unexpected reply: {e}")))
}

/// The last good entitlement, on disk, for the offline grace.
pub struct Cache {
    path: PathBuf,
}

impl Cache {
    /// `<dir>/entitlement.json`.
    pub fn new(dir: &Path) -> Cache {
        Cache::named(dir, "entitlement.json")
    }

    /// `<dir>/<file>`: one cache per server, so one server's answer is never replayed for
    /// another.
    pub fn named(dir: &Path, file: &str) -> Cache {
        Cache {
            path: dir.join(file),
        }
    }

    /// None when missing or unreadable.
    pub fn load(&self) -> Option<Entitlement> {
        serde_json::from_slice(&std::fs::read(&self.path).ok()?).ok()
    }

    pub fn clear(&self) {
        let _ = std::fs::remove_file(&self.path);
    }

    pub fn save(&self, e: &Entitlement) {
        let write = || -> std::io::Result<()> {
            if let Some(dir) = self.path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = self.path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec(e).map_err(std::io::Error::other)?)?;
            std::fs::rename(&tmp, &self.path)
        };
        if let Err(err) = write() {
            log::warn!("could not cache the entitlement: {err}");
        }
    }
}

#[derive(Debug, Clone)]
pub enum Decision {
    Allow(Entitlement),
    /// Allowed on a cached entitlement this old.
    AllowStale(Entitlement, Duration),
    Lock(LockReason, String),
    SignIn,
    /// The remembered studio no longer exists: pick another.
    ChooseStudio,
}

impl Decision {
    fn active(&self) -> Option<&Entitlement> {
        match self {
            Decision::Allow(e) | Decision::AllowStale(e, _) => {
                Some(e).filter(|e| e.standing == Standing::Active)
            }
            _ => None,
        }
    }

    pub fn ai_allowed(&self) -> bool {
        self.active().is_some_and(|e| e.ai.available)
    }

    /// The AI tools that run on this computer (denoise, upscale, masks, removal): the studio
    /// must be in good standing (false in `read_only` and unknown standings), as for any AI;
    /// they use none of the plan's server AI allowance.
    pub fn local_ai_allowed(&self) -> bool {
        self.active().is_some()
    }

    /// Export to a LensFlow gallery (false in `read_only` and unknown standings). Export to
    /// disk is always allowed while the app is unlocked and needs no check.
    pub fn gallery_export_allowed(&self) -> bool {
        self.active().is_some()
    }

    /// Preset sync; false in `read_only`.
    pub fn sync_allowed(&self) -> bool {
        self.active().is_some()
    }
}

pub struct Gate;

impl Gate {
    /// Pure decision from the cached and the fresh result for the studio `studio_id`. A
    /// cached entitlement for any other studio counts as no cache: one studio's answer is
    /// never replayed for another.
    pub fn evaluate(
        now: SystemTime,
        studio_id: u64,
        cached: Option<&Entitlement>,
        fresh: Result<Entitlement, AccountError>,
    ) -> Decision {
        let cached = cached.filter(|c| c.studio.id == studio_id);
        match fresh {
            Ok(e) => match Self::lock_for(&e) {
                Some((reason, msg)) => Decision::Lock(reason, msg),
                None => Decision::Allow(e),
            },
            Err(AccountError::Unauthorized) => Decision::SignIn,
            Err(AccountError::Locked(reason, msg)) => Decision::Lock(reason, msg),
            Err(AccountError::StudioNotFound) => Decision::ChooseStudio,
            Err(err) if !is_transient(&err) => {
                Decision::Lock(LockReason::Refused, refusal_message(&err))
            }
            Err(_) => match cached {
                Some(c) => {
                    if c.checked_at > now + CLOCK_SKEW {
                        return Self::offline();
                    }
                    let age = now.duration_since(c.checked_at).unwrap_or(Duration::ZERO);
                    if age > GRACE {
                        return Self::offline();
                    }
                    match Self::lock_for(c) {
                        Some((reason, msg)) => Decision::Lock(reason, msg),
                        None => Decision::AllowStale(c.clone(), age),
                    }
                }
                None => Self::offline(),
            },
        }
    }

    fn offline() -> Decision {
        Decision::Lock(LockReason::Offline, OFFLINE_MESSAGE.to_string())
    }

    fn lock_for(e: &Entitlement) -> Option<(LockReason, String)> {
        match (e.enabled, e.standing) {
            (false, _) => Some((
                LockReason::NotEnabled,
                "The desktop editor is not enabled for your studio. Ask LensFlow to switch it on."
                    .into(),
            )),
            (_, Standing::Paused) => Some((
                LockReason::Paused,
                "lensflow-ImageEditor is paused while your LensFlow subscription is paused. Resume your plan to use it again."
                    .into(),
            )),
            (_, Standing::Suspended) => Some((
                LockReason::Suspended,
                "This studio is suspended. Contact the platform owner.".into(),
            )),
            _ => None,
        }
    }

    /// Checks with the server for the studio `studio_id` (the client's `X-Studio`),
    /// updates the cache on success, and decides.
    pub fn refresh(client: &Client, cache: &Cache, studio_id: u64, now: SystemTime) -> Decision {
        let fresh = check(client);
        match &fresh {
            Ok(e) => cache.save(e),
            // A definite answer: never replay an older active entitlement offline.
            Err(
                AccountError::Locked(..)
                | AccountError::Unauthorized
                | AccountError::StudioNotFound,
            ) => cache.clear(),
            Err(_) => {}
        }
        let cached = cache.load();
        Self::evaluate(now, studio_id, cached.as_ref(), fresh)
    }
}

/// Only a hiccup may lean on the cached entitlement.
fn is_transient(err: &AccountError) -> bool {
    matches!(
        err,
        AccountError::Network(_)
            | AccountError::RateLimited
            | AccountError::BadReply(_)
            | AccountError::Server(500..=599, _)
    )
}

fn refusal_message(err: &AccountError) -> String {
    match err {
        AccountError::Server(_, m) | AccountError::ReadOnly(m) | AccountError::PlanLimit(m) => {
            m.clone()
        }
        other => other.to_string(),
    }
}
