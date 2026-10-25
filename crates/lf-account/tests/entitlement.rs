use httpmock::prelude::*;
use lf_account::entitlement::{self, Cache, Decision, Entitlement, Gate, Standing};
use lf_account::{AccountError, Client, Instance, LockReason, download_link};
use serde_json::json;
use std::time::{Duration, SystemTime};

const DAY: Duration = Duration::from_secs(86_400);

fn client(server: &MockServer) -> Client {
    let url = url::Url::parse(&server.url("/api")).unwrap();
    Client::new(Instance::Custom(url), "1.0.0")
        .with_token("t")
        .with_studio("frames")
}

fn body(enabled: bool, standing: &str) -> serde_json::Value {
    json!({"data": {
        "enabled": enabled, "standing": standing,
        "ai": {"available": true, "remaining": 12},
        "studio": {"id": 3, "name": "Frames", "logo_url": null},
        "min_version": "0.2.0",
        "latest": {"version": "0.3.0", "notes": "n", "published_at": "2026-10-01T00:00:00+00:00"}
    }})
}

fn ent(standing: Standing, enabled: bool, age: Duration) -> Entitlement {
    let mut e = Entitlement::example(standing, enabled);
    e.checked_at = SystemTime::now() - age;
    e
}

#[test]
fn check_parses_the_payload_and_sends_the_studio() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(GET)
            .path("/api/desktop/entitlement")
            .header("X-Studio", "frames")
            .header("Authorization", "Bearer t");
        then.status(200).json_body(body(true, "active"));
    });
    let e = entitlement::check(&client(&server)).unwrap();
    m.assert();
    assert!(e.enabled);
    assert_eq!(e.standing, Standing::Active);
    assert_eq!(e.ai.remaining, Some(12));
    assert_eq!(e.studio.name, "Frames");
    assert_eq!(e.min_version.as_deref(), Some("0.2.0"));
    assert_eq!(e.latest.unwrap().version, "0.3.0");
    assert!(e.checked_at.elapsed().unwrap() < Duration::from_secs(60));
}

#[test]
fn check_maps_each_refusal_code() {
    let server = MockServer::start();
    let c = client(&server);
    let mut m = server.mock(|when, then| {
        when.method(GET).path("/api/desktop/entitlement");
        then.status(403)
            .json_body(json!({"message": "suspended!", "code": "studio_suspended"}));
    });
    assert!(
        matches!(entitlement::check(&c).unwrap_err(), AccountError::Locked(LockReason::Suspended, m) if m == "suspended!")
    );
    m.delete();
    m = server.mock(|when, then| {
        when.method(GET).path("/api/desktop/entitlement");
        then.status(403)
            .json_body(json!({"message": "off", "code": "desktop_editor_disabled"}));
    });
    assert!(
        matches!(entitlement::check(&c).unwrap_err(), AccountError::Locked(LockReason::NotEnabled, m) if m == "off")
    );
    m.delete();
    server.mock(|when, then| {
        when.method(GET).path("/api/desktop/entitlement");
        then.status(402)
            .json_body(json!({"message": "paused", "code": "subscription_paused"}));
    });
    assert!(
        matches!(entitlement::check(&c).unwrap_err(), AccountError::Locked(LockReason::Paused, m) if m == "paused")
    );
}

#[test]
fn download_link_parses() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/api/desktop/download/windows");
        then.status(200).json_body(json!({"data": {"url": "https://blob/x", "sha256": "ab", "version": "0.3.0", "size": 1234, "expires_at": "2026-10-25T10:00:00+00:00"}}));
    });
    let l = download_link(&client(&server), "windows").unwrap();
    assert_eq!(
        (
            l.url.as_str(),
            l.sha256.as_str(),
            l.version.as_str(),
            l.size
        ),
        ("https://blob/x", "ab", "0.3.0", 1234)
    );
}

#[test]
fn fresh_active_allows() {
    let d = Gate::evaluate(
        SystemTime::now(),
        None,
        Ok(ent(Standing::Active, true, Duration::ZERO)),
    );
    assert!(matches!(d, Decision::Allow(_)));
    assert!(d.ai_allowed() && d.export_allowed() && d.sync_allowed());
}

#[test]
fn network_error_with_a_two_day_cache_is_allowed_stale() {
    let cached = ent(Standing::Active, true, 2 * DAY);
    let d = Gate::evaluate(
        SystemTime::now(),
        Some(&cached),
        Err(AccountError::Network("down".into())),
    );
    match &d {
        Decision::AllowStale(_, age) => assert!(*age >= 2 * DAY && *age < 3 * DAY),
        other => panic!("{other:?}"),
    }
    assert!(d.export_allowed());
}

#[test]
fn server_error_counts_like_a_network_error() {
    let cached = ent(Standing::Active, true, DAY);
    let d = Gate::evaluate(
        SystemTime::now(),
        Some(&cached),
        Err(AccountError::Server(503, "x".into())),
    );
    assert!(matches!(d, Decision::AllowStale(..)));
}

#[test]
fn network_error_with_an_eight_day_cache_locks_offline() {
    let cached = ent(Standing::Active, true, 8 * DAY);
    let d = Gate::evaluate(
        SystemTime::now(),
        Some(&cached),
        Err(AccountError::Network("down".into())),
    );
    assert!(
        matches!(&d, Decision::Lock(LockReason::Offline, m) if m == "Connect to the internet to keep using lensflow-ImageEditor.")
    );
    assert!(!d.export_allowed());
}

#[test]
fn network_error_without_a_cache_locks_offline() {
    let d = Gate::evaluate(
        SystemTime::now(),
        None,
        Err(AccountError::Network("down".into())),
    );
    assert!(matches!(d, Decision::Lock(LockReason::Offline, _)));
}

#[test]
fn stale_read_only_cache_keeps_its_flags_off() {
    let cached = ent(Standing::ReadOnly, true, DAY);
    let d = Gate::evaluate(
        SystemTime::now(),
        Some(&cached),
        Err(AccountError::Network("down".into())),
    );
    assert!(matches!(d, Decision::AllowStale(..)));
    assert!(!d.ai_allowed() && !d.export_allowed() && !d.sync_allowed());
}

#[test]
fn stale_cache_that_was_locked_stays_locked() {
    let cached = ent(Standing::Paused, true, DAY);
    let d = Gate::evaluate(
        SystemTime::now(),
        Some(&cached),
        Err(AccountError::Network("down".into())),
    );
    assert!(matches!(d, Decision::Lock(LockReason::Paused, _)));
}

#[test]
fn fresh_locked_error_locks_with_the_server_message() {
    let cached = ent(Standing::Active, true, Duration::ZERO);
    let d = Gate::evaluate(
        SystemTime::now(),
        Some(&cached),
        Err(AccountError::Locked(
            LockReason::Paused,
            "Resume your plan.".into(),
        )),
    );
    assert!(matches!(&d, Decision::Lock(LockReason::Paused, m) if m == "Resume your plan."));
}

#[test]
fn fresh_paused_standing_locks() {
    let d = Gate::evaluate(
        SystemTime::now(),
        None,
        Ok(ent(Standing::Paused, true, Duration::ZERO)),
    );
    assert!(matches!(d, Decision::Lock(LockReason::Paused, _)));
    let d = Gate::evaluate(
        SystemTime::now(),
        None,
        Ok(ent(Standing::Suspended, true, Duration::ZERO)),
    );
    assert!(matches!(d, Decision::Lock(LockReason::Suspended, _)));
}

#[test]
fn fresh_disabled_locks_not_enabled() {
    let d = Gate::evaluate(
        SystemTime::now(),
        None,
        Ok(ent(Standing::Active, false, Duration::ZERO)),
    );
    assert!(matches!(d, Decision::Lock(LockReason::NotEnabled, _)));
}

#[test]
fn read_only_allows_with_ai_export_and_sync_off() {
    let d = Gate::evaluate(
        SystemTime::now(),
        None,
        Ok(ent(Standing::ReadOnly, true, Duration::ZERO)),
    );
    assert!(matches!(d, Decision::Allow(_)));
    assert!(!d.ai_allowed() && !d.export_allowed() && !d.sync_allowed());
}

#[test]
fn ai_needs_availability_too() {
    let mut e = ent(Standing::Active, true, Duration::ZERO);
    e.ai.available = false;
    let d = Gate::evaluate(SystemTime::now(), None, Ok(e));
    assert!(!d.ai_allowed() && d.export_allowed());
}

#[test]
fn fresh_unauthorized_means_sign_in() {
    let cached = ent(Standing::Active, true, DAY);
    let d = Gate::evaluate(
        SystemTime::now(),
        Some(&cached),
        Err(AccountError::Unauthorized),
    );
    assert!(matches!(d, Decision::SignIn));
    assert!(!d.export_allowed());
}

#[test]
fn cache_round_trips_and_refresh_updates_it_on_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path());
    assert!(cache.load().is_none());
    let server = MockServer::start();
    let mut m = server.mock(|when, then| {
        when.method(GET).path("/api/desktop/entitlement");
        then.status(200).json_body(body(true, "active"));
    });
    let d = Gate::refresh(&client(&server), &cache, SystemTime::now());
    assert!(matches!(d, Decision::Allow(_)));
    let saved = cache.load().expect("saved on fresh ok");
    assert_eq!(saved.studio.name, "Frames");
    // The server goes away: the cache carries us.
    m.delete();
    server.mock(|when, then| {
        when.method(GET).path("/api/desktop/entitlement");
        then.status(503);
    });
    assert!(matches!(
        Gate::refresh(&client(&server), &cache, SystemTime::now()),
        Decision::AllowStale(..)
    ));
    // A corrupt cache reads as none.
    std::fs::write(dir.path().join("entitlement.json"), "{nope").unwrap();
    assert!(cache.load().is_none());
}

fn refuse_mock(server: &MockServer, status: u16, body: serde_json::Value) {
    server.mock(|when, then| {
        when.method(GET).path("/api/desktop/entitlement");
        then.status(status).json_body(body);
    });
}

#[test]
fn a_403_without_a_code_locks_as_refused_even_with_a_fresh_cache() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path());
    cache.save(&ent(Standing::Active, true, Duration::ZERO));
    let server = MockServer::start();
    refuse_mock(&server, 403, json!({"message": "You may not do that."}));
    let d = Gate::refresh(&client(&server), &cache, SystemTime::now());
    assert!(
        matches!(&d, Decision::Lock(LockReason::Refused, m) if m == "You may not do that."),
        "{d:?}"
    );
}

#[test]
fn studio_not_found_asks_to_choose_a_studio() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path());
    cache.save(&ent(Standing::Active, true, Duration::ZERO));
    let server = MockServer::start();
    refuse_mock(
        &server,
        404,
        json!({"message": "Studio not found.", "code": "studio_not_found"}),
    );
    let d = Gate::refresh(&client(&server), &cache, SystemTime::now());
    assert!(matches!(d, Decision::ChooseStudio), "{d:?}");
    assert!(!d.export_allowed());
    assert!(cache.load().is_none());
}

#[test]
fn definite_refusals_do_not_use_the_grace() {
    let cached = ent(Standing::Active, true, DAY);
    for err in [
        AccountError::Server(404, "gone".into()),
        AccountError::Server(400, "bad".into()),
        AccountError::Server(200, "unexpected reply".into()),
        AccountError::Validation(Default::default()),
    ] {
        let d = Gate::evaluate(SystemTime::now(), Some(&cached), Err(err));
        assert!(matches!(d, Decision::Lock(LockReason::Refused, _)), "{d:?}");
    }
    for err in [
        AccountError::RateLimited,
        AccountError::Server(502, "x".into()),
    ] {
        let d = Gate::evaluate(SystemTime::now(), Some(&cached), Err(err));
        assert!(matches!(d, Decision::AllowStale(..)), "{d:?}");
    }
}

#[test]
fn fresh_locked_then_network_error_stays_locked() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path());
    let server = MockServer::start();
    let mut ok = server.mock(|when, then| {
        when.method(GET).path("/api/desktop/entitlement");
        then.status(200).json_body(body(true, "active"));
    });
    assert!(matches!(
        Gate::refresh(&client(&server), &cache, SystemTime::now()),
        Decision::Allow(_)
    ));
    ok.delete();
    let mut locked = server.mock(|when, then| {
        when.method(GET).path("/api/desktop/entitlement");
        then.status(402)
            .json_body(json!({"message": "paused", "code": "subscription_paused"}));
    });
    assert!(matches!(
        Gate::refresh(&client(&server), &cache, SystemTime::now()),
        Decision::Lock(LockReason::Paused, _)
    ));
    locked.delete();
    server.mock(|when, then| {
        when.method(GET).path("/api/desktop/entitlement");
        then.status(503);
    });
    let d = Gate::refresh(&client(&server), &cache, SystemTime::now());
    assert!(matches!(d, Decision::Lock(LockReason::Offline, _)), "{d:?}");
}

#[test]
fn unauthorized_clears_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path());
    cache.save(&ent(Standing::Active, true, Duration::ZERO));
    let server = MockServer::start();
    refuse_mock(&server, 401, json!({"message": "Unauthenticated."}));
    assert!(matches!(
        Gate::refresh(&client(&server), &cache, SystemTime::now()),
        Decision::SignIn
    ));
    assert!(cache.load().is_none());
}

#[test]
fn a_cache_dated_in_the_future_is_expired() {
    let mut cached = Entitlement::example(Standing::Active, true);
    cached.checked_at = SystemTime::now() + Duration::from_secs(3600);
    let d = Gate::evaluate(
        SystemTime::now(),
        Some(&cached),
        Err(AccountError::Network("x".into())),
    );
    assert!(matches!(d, Decision::Lock(LockReason::Offline, _)), "{d:?}");
    // A little clock skew is tolerated.
    cached.checked_at = SystemTime::now() + Duration::from_secs(60);
    let d = Gate::evaluate(
        SystemTime::now(),
        Some(&cached),
        Err(AccountError::Network("x".into())),
    );
    assert!(matches!(d, Decision::AllowStale(..)), "{d:?}");
}

#[test]
fn a_cache_without_checked_at_is_ancient() {
    let dir = tempfile::tempdir().unwrap();
    let mut v = serde_json::to_value(Entitlement::example(Standing::Active, true)).unwrap();
    v.as_object_mut().unwrap().remove("checked_at");
    std::fs::write(dir.path().join("entitlement.json"), v.to_string()).unwrap();
    let cached = Cache::new(dir.path()).load().unwrap();
    let d = Gate::evaluate(
        SystemTime::now(),
        Some(&cached),
        Err(AccountError::Network("x".into())),
    );
    assert!(matches!(d, Decision::Lock(LockReason::Offline, _)), "{d:?}");
}
