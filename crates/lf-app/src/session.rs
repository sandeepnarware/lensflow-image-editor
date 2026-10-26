//! Who is signed in and whether the app may be used: a pure state machine fed by
//! `lf_account`'s answers. The screens follow from the current [`Session`]; nothing here
//! touches the network or the disk.

use crate::version;
use lf_account::LockReason;
use lf_account::auth::{StudioSummary, User};
use lf_account::entitlement::{Decision, Entitlement, Release, Standing};
use std::time::{Duration, Instant};

/// Shown on the sign-in screen after any call answers 401.
pub const SIGNED_OUT_MESSAGE: &str = "You were signed out. Sign in again to continue.";
pub const PAUSED_MESSAGE: &str =
    "Your studio is paused. Resume your LensFlow plan to use lensflow-ImageEditor.";
pub const SUSPENDED_MESSAGE: &str =
    "Your studio is suspended. Contact LensFlow support to continue.";
pub const NOT_ENABLED_MESSAGE: &str =
    "Your studio does not have lensflow-ImageEditor yet. Ask LensFlow support to enable it.";
pub const NO_PERMISSION_MESSAGE: &str = "Your studio's owner can give you access to lensflow-ImageEditor. Ask for the 'manage galleries' permission.";
pub const READ_ONLY_BANNER: &str =
    "Your LensFlow plan has lapsed: editing works, but preset sync and AI are off until you renew.";
pub const NO_STUDIO_MESSAGE: &str =
    "Your account is not part of a studio yet. Ask your studio's owner to invite you.";

/// How often the entitlement is checked while the app is open.
pub const GATE_INTERVAL: Duration = Duration::from_secs(4 * 60 * 60);

// `Active` carries the entitlement and is much larger than the other states; there is
// only ever one `Session`, so boxing it would buy nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Clone)]
pub enum Session {
    SigningIn {
        email: String,
        password: String,
        code: String,
        two_factor: bool,
        busy: bool,
        error: Option<String>,
    },
    ChoosingStudio {
        user: User,
    },
    /// The chosen studio has not given this person the desktop permission.
    NoPermission {
        studio: String,
    },
    Locked {
        reason: LockReason,
        message: String,
    },
    UpdateRequired {
        latest: Option<Release>,
    },
    Active {
        studio: StudioSummary,
        entitlement: Entitlement,
        /// Allowed on a cached entitlement this old (the server could not be reached).
        stale: Option<Duration>,
        /// The plan has lapsed: local editing and disk export only.
        read_only: bool,
        /// A newer release the user may install.
        update: Option<Release>,
    },
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // The password and the two-factor code never print.
            Session::SigningIn {
                email,
                two_factor,
                busy,
                error,
                ..
            } => f
                .debug_struct("SigningIn")
                .field("email", email)
                .field("two_factor", two_factor)
                .field("busy", busy)
                .field("error", error)
                .finish_non_exhaustive(),
            Session::ChoosingStudio { user } => f
                .debug_struct("ChoosingStudio")
                .field("studios", &user.studios.len())
                .finish(),
            Session::NoPermission { studio } => f
                .debug_struct("NoPermission")
                .field("studio", studio)
                .finish(),
            Session::Locked { reason, message } => f
                .debug_struct("Locked")
                .field("reason", reason)
                .field("message", message)
                .finish(),
            Session::UpdateRequired { latest } => f
                .debug_struct("UpdateRequired")
                .field("latest", latest)
                .finish(),
            Session::Active {
                studio,
                entitlement,
                stale,
                read_only,
                update,
            } => f
                .debug_struct("Active")
                .field("studio", &studio.slug)
                .field("standing", &entitlement.standing)
                .field("stale", stale)
                .field("read_only", read_only)
                .field("update", update)
                .finish(),
        }
    }
}

/// What to do once the user's studios are known.
#[derive(Debug, Clone)]
pub enum StudioChoice {
    /// One studio, or the remembered one: use it.
    Chosen(StudioSummary),
    /// Several studios and none remembered: show the chooser.
    Ask,
    /// The account belongs to no studio.
    None,
}

/// The remembered studio when it is still one of the user's, else the only one, else ask.
pub fn choose_studio(user: &User, remembered: Option<&str>) -> StudioChoice {
    let remembered = remembered.and_then(|slug| user.studios.iter().find(|s| s.slug == slug));
    match (remembered, user.studios.as_slice()) {
        (Some(studio), _) | (None, [studio]) => StudioChoice::Chosen(studio.clone()),
        (None, []) => StudioChoice::None,
        (None, _) => StudioChoice::Ask,
    }
}

impl Session {
    /// An empty sign-in form.
    pub fn signing_in() -> Session {
        Session::SigningIn {
            email: String::new(),
            password: String::new(),
            code: String::new(),
            two_factor: false,
            busy: false,
            error: None,
        }
    }

    /// The session after the user picked `studio` (before any entitlement call): `None`
    /// means go on and check the entitlement.
    pub fn on_studio_chosen(studio: &StudioSummary) -> Option<Session> {
        (!studio.can_use_desktop()).then(|| Session::NoPermission {
            studio: studio.name.clone(),
        })
    }

    /// The entitlement gate answered for `studio`, one of `user`'s studios.
    pub fn on_decision(
        self,
        decision: Decision,
        user: &User,
        studio: &StudioSummary,
        current_version: &str,
    ) -> Session {
        match decision {
            Decision::Allow(entitlement) => allowed(studio, entitlement, None, current_version),
            Decision::AllowStale(entitlement, age) => {
                allowed(studio, entitlement, Some(age), current_version)
            }
            Decision::Lock(LockReason::Refused, _) if !studio.can_use_desktop() => {
                Session::NoPermission {
                    studio: studio.name.clone(),
                }
            }
            Decision::Lock(reason, message) => Session::Locked {
                reason,
                message: lock_message(reason, message),
            },
            Decision::SignIn => self.on_unauthorized(),
            Decision::ChooseStudio => {
                let mut user = user.clone();
                user.studios.retain(|s| s.slug != studio.slug);
                Session::ChoosingStudio { user }
            }
        }
    }

    /// A call answered 401: back to sign-in with [`SIGNED_OUT_MESSAGE`]. The account lane
    /// clears the stored token and the cached entitlement (`account::forget`).
    pub fn on_unauthorized(self) -> Session {
        let email = match self {
            Session::SigningIn { email, .. } => email,
            _ => String::new(),
        };
        Session::SigningIn {
            email,
            password: String::new(),
            code: String::new(),
            two_factor: false,
            busy: false,
            error: Some(SIGNED_OUT_MESSAGE.to_owned()),
        }
    }
}

/// `Allow` or `AllowStale`: the editor, unless this version is below the studio's minimum.
fn allowed(
    studio: &StudioSummary,
    entitlement: Entitlement,
    stale: Option<Duration>,
    current_version: &str,
) -> Session {
    let too_old = entitlement
        .min_version
        .as_deref()
        .is_some_and(|min| version::newer(min, current_version));
    if too_old {
        return Session::UpdateRequired {
            latest: entitlement.latest,
        };
    }
    let update = entitlement
        .latest
        .clone()
        .filter(|release| version::newer(&release.version, current_version));
    // The gate locks paused and suspended studios, so only Active reaches here with full
    // rights; ReadOnly, Unknown and any standing added later run read-only.
    let read_only = !matches!(entitlement.standing, Standing::Active);
    Session::Active {
        studio: studio.clone(),
        entitlement,
        stale,
        read_only,
        update,
    }
}

/// The app's own copy for the standings it knows; the library's message otherwise.
fn lock_message(reason: LockReason, library_message: String) -> String {
    match reason {
        LockReason::Paused => PAUSED_MESSAGE.to_owned(),
        LockReason::Suspended => SUSPENDED_MESSAGE.to_owned(),
        LockReason::NotEnabled => NOT_ENABLED_MESSAGE.to_owned(),
        LockReason::Refused | LockReason::Offline => library_message,
    }
}

/// When the next entitlement check is due.
pub struct GateClock;

impl GateClock {
    /// No check yet, or the last one was [`GATE_INTERVAL`] ago or more.
    pub fn due(last: Option<Instant>, now: Instant) -> bool {
        last.is_none_or(|last| now.saturating_duration_since(last) >= GATE_INTERVAL)
    }
}

/// "Last checked just now", "… 5 minutes ago", "… 3 hours ago".
pub fn last_checked(age: Duration) -> String {
    let minutes = age.as_secs() / 60;
    let hours = minutes / 60;
    match (hours, minutes) {
        (0, 0) => "Last checked just now".to_owned(),
        (0, 1) => "Last checked 1 minute ago".to_owned(),
        (0, m) => format!("Last checked {m} minutes ago"),
        (1, _) => "Last checked 1 hour ago".to_owned(),
        (h, _) => format!("Last checked {h} hours ago"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::Credentials;
    use lf_account::Instance;
    use serde_json::json;

    const CURRENT: &str = "1.2.0";

    fn studio(permissions: &[&str]) -> StudioSummary {
        serde_json::from_value(json!({
            "id": 7, "name": "Frames", "slug": "frames", "role": "editor",
            "permissions": permissions,
        }))
        .unwrap()
    }

    fn user(studios: &[StudioSummary]) -> User {
        let studios: Vec<_> = studios
            .iter()
            .map(|s| json!({"id": s.id, "name": s.name, "slug": s.slug, "role": s.role, "permissions": s.permissions}))
            .collect();
        serde_json::from_value(
            json!({"id": 1, "name": "Asha", "email": "asha@example.com", "studios": studios}),
        )
        .unwrap()
    }

    fn allowed() -> StudioSummary {
        studio(&["galleries.manage"])
    }

    fn active_session() -> Session {
        let s = allowed();
        Session::signing_in().on_decision(
            Decision::Allow(Entitlement::example(Standing::Active, true)),
            &user(std::slice::from_ref(&s)),
            &s,
            CURRENT,
        )
    }

    #[test]
    fn allow_active_goes_to_active() {
        match active_session() {
            Session::Active {
                studio,
                stale,
                read_only,
                update,
                entitlement,
            } => {
                assert_eq!(studio.slug, "frames");
                assert_eq!(stale, None);
                assert!(!read_only);
                assert_eq!(update, None);
                assert_eq!(entitlement.standing, Standing::Active);
            }
            other => panic!("expected Active, got {other:?}"),
        }
    }

    #[test]
    fn allow_read_only_sets_banner_and_keeps_editing() {
        let s = allowed();
        let decision = Decision::Allow(Entitlement::example(Standing::ReadOnly, true));
        assert!(!decision.ai_allowed());
        assert!(!decision.sync_allowed());
        let next = Session::signing_in().on_decision(
            decision,
            &user(std::slice::from_ref(&s)),
            &s,
            CURRENT,
        );
        assert!(
            matches!(
                next,
                Session::Active {
                    read_only: true,
                    ..
                }
            ),
            "read-only still opens the editor: {next:?}"
        );
        let unknown = Decision::Allow(Entitlement::example(Standing::Unknown, true));
        assert!(!unknown.gallery_export_allowed());
        let next = Session::signing_in().on_decision(
            unknown,
            &user(std::slice::from_ref(&s)),
            &s,
            CURRENT,
        );
        assert!(
            matches!(
                next,
                Session::Active {
                    read_only: true,
                    ..
                }
            ),
            "unknown standing runs read-only"
        );
        assert_eq!(
            READ_ONLY_BANNER,
            "Your LensFlow plan has lapsed: editing works, but preset sync and AI are off until you renew."
        );
    }

    #[test]
    fn stale_keeps_age() {
        let s = allowed();
        let age = Duration::from_secs(5 * 3600);
        let next = Session::signing_in().on_decision(
            Decision::AllowStale(Entitlement::example(Standing::Active, true), age),
            &user(std::slice::from_ref(&s)),
            &s,
            CURRENT,
        );
        assert!(
            matches!(next, Session::Active { stale: Some(a), read_only: false, .. } if a == age)
        );
        assert_eq!(last_checked(age), "Last checked 5 hours ago");
        assert_eq!(
            last_checked(Duration::from_secs(3600)),
            "Last checked 1 hour ago"
        );
        assert_eq!(
            last_checked(Duration::from_secs(20)),
            "Last checked just now"
        );
        assert_eq!(
            last_checked(Duration::from_secs(120)),
            "Last checked 2 minutes ago"
        );
    }

    #[test]
    fn lock_paused_uses_owner_copy() {
        let s = allowed();
        let u = user(std::slice::from_ref(&s));
        let lock = |reason, msg: &str| {
            Session::signing_in().on_decision(Decision::Lock(reason, msg.into()), &u, &s, CURRENT)
        };
        let message_of = |session: Session| match session {
            Session::Locked { message, .. } => message,
            other => panic!("expected Locked, got {other:?}"),
        };
        assert_eq!(
            message_of(lock(LockReason::Paused, "library copy")),
            "Your studio is paused. Resume your LensFlow plan to use lensflow-ImageEditor."
        );
        assert_eq!(
            message_of(lock(LockReason::Suspended, "library copy")),
            "Your studio is suspended. Contact LensFlow support to continue."
        );
        assert_eq!(
            message_of(lock(LockReason::NotEnabled, "library copy")),
            "Your studio does not have lensflow-ImageEditor yet. Ask LensFlow support to enable it."
        );
        // Offline past grace and other refusals keep the library's own message.
        assert_eq!(
            message_of(lock(LockReason::Offline, "Connect to the internet.")),
            "Connect to the internet."
        );
        assert_eq!(
            message_of(lock(LockReason::Refused, "Not today.")),
            "Not today."
        );
    }

    #[test]
    fn refused_without_permission_is_no_permission() {
        let viewer = studio(&["galleries.view"]);
        let next = Session::signing_in().on_decision(
            Decision::Lock(LockReason::Refused, "This action is unauthorized.".into()),
            &user(std::slice::from_ref(&viewer)),
            &viewer,
            CURRENT,
        );
        assert!(matches!(&next, Session::NoPermission { studio } if studio == "Frames"));
        assert_eq!(
            NO_PERMISSION_MESSAGE,
            "Your studio's owner can give you access to lensflow-ImageEditor. Ask for the 'manage galleries' permission."
        );
        // The chooser stops such a studio before any entitlement call.
        assert!(matches!(
            Session::on_studio_chosen(&viewer),
            Some(Session::NoPermission { .. })
        ));
        assert!(Session::on_studio_chosen(&allowed()).is_none());
    }

    fn mock_client(server: &httpmock::MockServer) -> lf_account::Client {
        let url = reqwest::Url::parse(&server.url("/api")).unwrap();
        lf_account::Client::new(Instance::custom(url).unwrap(), CURRENT)
            .with_token("secret-token")
            .with_studio("frames")
    }

    fn stored(dir: &std::path::Path) -> Credentials {
        let creds = Credentials::in_dir(&Instance::Dev, dir);
        creds.save_token("secret-token");
        creds
            .cache()
            .save(&Entitlement::example(Standing::Active, true));
        assert_eq!(creds.token().as_deref(), Some("secret-token"));
        creds
    }

    #[test]
    fn sign_in_decision_clears_token() {
        let dir = tempfile::tempdir().unwrap();
        let creds = stored(dir.path());
        let server = httpmock::MockServer::start();
        server.mock(|when, then| {
            when.path("/api/desktop/entitlement");
            then.status(401)
                .json_body(json!({"message": "Unauthenticated."}));
        });

        // The account lane's gate job forgets the credentials before it answers.
        let decision = match crate::account::gate(5, &mock_client(&server), &creds, 1) {
            crate::jobs::Msg::Gate(5, decision) => decision,
            other => panic!("expected Gate(5, ..), got {other:?}"),
        };
        assert!(matches!(decision, Decision::SignIn));
        assert_eq!(creds.token(), None, "the token is cleared");
        assert!(creds.cache().load().is_none(), "the cache is cleared");

        let s = allowed();
        let next =
            active_session().on_decision(decision, &user(std::slice::from_ref(&s)), &s, CURRENT);
        match next {
            Session::SigningIn {
                password,
                error,
                busy,
                ..
            } => {
                assert!(password.is_empty());
                assert!(!busy);
                assert_eq!(error.as_deref(), Some(SIGNED_OUT_MESSAGE));
            }
            other => panic!("expected SigningIn, got {other:?}"),
        }
    }

    #[test]
    fn min_version_newer_blocks() {
        let s = allowed();
        let mut e = Entitlement::example(Standing::Active, true);
        e.min_version = Some("1.3.0".into());
        e.latest = Some(Release {
            version: "1.4.0".into(),
            notes: None,
            published_at: None,
        });
        let next = Session::signing_in().on_decision(
            Decision::Allow(e.clone()),
            &user(std::slice::from_ref(&s)),
            &s,
            CURRENT,
        );
        assert!(
            matches!(&next, Session::UpdateRequired { latest: Some(r) } if r.version == "1.4.0")
        );

        e.min_version = Some("1.2.0".into());
        let same = Session::signing_in().on_decision(
            Decision::Allow(e),
            &user(std::slice::from_ref(&s)),
            &s,
            CURRENT,
        );
        assert!(
            matches!(same, Session::Active { .. }),
            "an equal minimum lets the app run"
        );
    }

    #[test]
    fn latest_newer_offers_update() {
        let s = allowed();
        let mut e = Entitlement::example(Standing::Active, true);
        e.latest = Some(Release {
            version: "1.3.0".into(),
            notes: None,
            published_at: None,
        });
        let next = Session::signing_in().on_decision(
            Decision::Allow(e.clone()),
            &user(std::slice::from_ref(&s)),
            &s,
            CURRENT,
        );
        assert!(matches!(&next, Session::Active { update: Some(r), .. } if r.version == "1.3.0"));

        e.latest.as_mut().unwrap().version = CURRENT.into();
        let current = Session::signing_in().on_decision(
            Decision::Allow(e),
            &user(std::slice::from_ref(&s)),
            &s,
            CURRENT,
        );
        assert!(matches!(current, Session::Active { update: None, .. }));
    }

    #[test]
    fn unauthorized_mid_session_returns_to_sign_in_with_message() {
        let next = active_session().on_unauthorized();
        assert!(matches!(
            &next,
            Session::SigningIn { error: Some(m), two_factor: false, busy: false, .. }
                if m == "You were signed out. Sign in again to continue."
        ));

        // A server hiccup on `auth/me` keeps the credentials ...
        let dir = tempfile::tempdir().unwrap();
        let creds = stored(dir.path());
        let server = httpmock::MockServer::start();
        let mut hiccup = server.mock(|when, then| {
            when.path("/api/auth/me");
            then.status(503);
        });
        let msg = crate::account::me(2, &mock_client(&server), &creds);
        assert!(matches!(msg, crate::jobs::Msg::Me(2, Err(_))));
        assert_eq!(creds.token().as_deref(), Some("secret-token"));
        assert!(creds.cache().load().is_some());

        // ... a 401 forgets both on the account lane.
        hiccup.delete();
        server.mock(|when, then| {
            when.path("/api/auth/me");
            then.status(401)
                .json_body(json!({"message": "Unauthenticated."}));
        });
        let msg = crate::account::me(2, &mock_client(&server), &creds);
        assert!(matches!(
            msg,
            crate::jobs::Msg::Me(2, Err(lf_account::AccountError::Unauthorized))
        ));
        assert_eq!(creds.token(), None);
        assert!(creds.cache().load().is_none());

        // The app's own forget job (a 401 from any other call) does the same.
        let creds = stored(dir.path());
        assert!(matches!(
            crate::account::forget(&creds),
            crate::jobs::Msg::Cleared
        ));
        assert_eq!(creds.token(), None);
        assert!(creds.cache().load().is_none());
    }

    #[test]
    fn choose_studio_prefers_remembered_then_single() {
        let a = allowed();
        let mut b = allowed();
        b.slug = "other".into();
        b.name = "Other".into();
        assert!(
            matches!(choose_studio(&user(std::slice::from_ref(&a)), None), StudioChoice::Chosen(s) if s.slug == "frames")
        );
        assert!(matches!(
            choose_studio(&user(&[a.clone(), b.clone()]), None),
            StudioChoice::Ask
        ));
        assert!(
            matches!(choose_studio(&user(&[a.clone(), b.clone()]), Some("other")), StudioChoice::Chosen(s) if s.slug == "other")
        );
        assert!(matches!(
            choose_studio(&user(&[a.clone(), b]), Some("gone")),
            StudioChoice::Ask
        ));
        assert!(matches!(
            choose_studio(&user(&[]), None),
            StudioChoice::None
        ));
    }

    #[test]
    fn choose_studio_decision_drops_the_missing_studio() {
        let a = allowed();
        let mut b = allowed();
        b.slug = "other".into();
        let next = active_session().on_decision(
            Decision::ChooseStudio,
            &user(&[a.clone(), b]),
            &a,
            CURRENT,
        );
        match next {
            Session::ChoosingStudio { user } => {
                let slugs: Vec<_> = user.studios.iter().map(|s| s.slug.as_str()).collect();
                assert_eq!(slugs, ["other"]);
            }
            other => panic!("expected ChoosingStudio, got {other:?}"),
        }
    }

    #[test]
    fn gate_due_after_four_hours() {
        let start = Instant::now();
        assert!(GateClock::due(None, start));
        assert!(!GateClock::due(Some(start), start));
        assert!(!GateClock::due(
            Some(start),
            start + Duration::from_secs(4 * 3600 - 1)
        ));
        assert!(GateClock::due(
            Some(start),
            start + Duration::from_secs(4 * 3600)
        ));
    }

    #[test]
    fn debug_never_prints_the_password() {
        let mut s = Session::signing_in();
        if let Session::SigningIn { password, .. } = &mut s {
            *password = "hunter2".into();
        }
        assert!(!format!("{s:?}").contains("hunter2"));
    }
}
