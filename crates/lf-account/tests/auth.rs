use httpmock::prelude::*;
use lf_account::{
    AccountError, Client, Instance, LockReason, TokenStore, auth, handle_unauthorized,
};
use serde_json::json;

fn client(server: &MockServer) -> Client {
    let url = url::Url::parse(&server.url("/api")).unwrap();
    Client::new(Instance::Custom(url), "1.2.3")
}

fn user_json() -> serde_json::Value {
    json!({
        "id": 7, "name": "Asha", "email": "asha@example.com", "is_platform_admin": false,
        "studios": [{
            "id": 3, "name": "Frames", "slug": "frames", "role": "owner",
            "permissions": ["galleries.view"], "plan_features": ["desktop_editor"],
            "subscription_paused": false, "status": "active", "brand_color": null
        }]
    })
}

#[test]
fn sign_in_sends_headers_and_body_and_parses_the_user() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST)
            .path("/api/auth/login")
            .header("X-LensFlow-Client", "desktop")
            .header("Accept", "application/json")
            .header_matches("User-Agent", r"^lensflow-ImageEditor/1\.2\.3 \(.+\)$")
            .json_body(
                json!({"email": "asha@example.com", "password": "pw", "device_name": "Asha PC"}),
            );
        then.status(200)
            .json_body(json!({"token": "tok-1", "user": user_json()}));
    });
    let signed =
        auth::sign_in(&client(&server), "asha@example.com", "pw", None, "Asha PC").unwrap();
    m.assert();
    assert_eq!(signed.token, "tok-1");
    assert_eq!(signed.user.name, "Asha");
    let s = &signed.user.studios[0];
    assert_eq!((s.slug.as_str(), s.role.as_str()), ("frames", "owner"));
    assert_eq!(s.plan_features, vec!["desktop_editor"]);
    assert!(!s.subscription_paused);
}

#[test]
fn two_factor_round_trip() {
    let server = MockServer::start();
    let first = server.mock(|when, then| {
        when.method(POST)
            .path("/api/auth/login")
            .json_body(json!({"email": "a@b.in", "password": "pw", "device_name": "pc"}));
        then.status(200)
            .json_body(json!({"two_factor_required": true}));
    });
    let second = server.mock(|when, then| {
        when.method(POST).path("/api/auth/login").json_body(json!({"email": "a@b.in", "password": "pw", "two_factor_code": "123456", "device_name": "pc"}));
        then.status(200).json_body(json!({"token": "tok-2", "user": user_json()}));
    });
    let c = client(&server);
    let err = auth::sign_in(&c, "a@b.in", "pw", None, "pc").unwrap_err();
    assert!(matches!(err, AccountError::TwoFactorRequired));
    let ok = auth::sign_in(&c, "a@b.in", "pw", Some("123456"), "pc").unwrap();
    assert_eq!(ok.token, "tok-2");
    first.assert();
    second.assert();
}

#[test]
fn validation_errors_map_field_to_message() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/api/auth/login");
        then.status(422).json_body(json!({"message": "x", "errors": {"email": ["These credentials do not match our records."]}}));
    });
    let err = auth::sign_in(&client(&server), "a@b.in", "bad", None, "pc").unwrap_err();
    match err {
        AccountError::Validation(map) => {
            assert_eq!(map["email"], "These credentials do not match our records.");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn me_sends_bearer_and_studio_headers() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(GET)
            .path("/api/auth/me")
            .header("Authorization", "Bearer tok-1")
            .header("X-Studio", "frames");
        then.status(200).json_body(json!({"data": user_json()}));
    });
    let c = client(&server).with_token("tok-1").with_studio("frames");
    let user = auth::me(&c).unwrap();
    m.assert();
    assert_eq!(user.email, "asha@example.com");
}

#[test]
fn a_401_is_unauthorized_and_handle_unauthorized_clears_the_token() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/api/auth/me");
        then.status(401)
            .json_body(json!({"message": "Unauthenticated."}));
    });
    let dir = tempfile::tempdir().unwrap();
    let store = TokenStore::open_in("https://x.test/api", dir.path(), false);
    store.save("tok-1");
    assert_eq!(store.load().as_deref(), Some("tok-1"));
    let err = auth::me(&client(&server).with_token("tok-1")).unwrap_err();
    assert!(matches!(err, AccountError::Unauthorized));
    assert!(handle_unauthorized(&store, &err));
    assert_eq!(store.load(), None);
    // Other errors leave the token alone.
    store.save("tok-2");
    assert!(!handle_unauthorized(&store, &AccountError::RateLimited));
    assert_eq!(store.load().as_deref(), Some("tok-2"));
}

#[test]
fn sign_out_ignores_failures() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST)
            .path("/api/auth/logout")
            .header("Authorization", "Bearer t");
        then.status(500);
    });
    auth::sign_out(&client(&server).with_token("t"));
    m.assert();
    // Unreachable server too.
    let dead = Client::new(
        Instance::Custom(url::Url::parse("http://127.0.0.1:1/api").unwrap()),
        "1",
    );
    auth::sign_out(&dead.with_token("t"));
}

#[test]
fn status_codes_map_to_typed_errors() {
    let cases: Vec<(u16, serde_json::Value)> = vec![
        (403, json!({"message": "m1", "code": "studio_suspended"})),
        (
            403,
            json!({"message": "m2", "code": "desktop_editor_disabled"}),
        ),
        (402, json!({"message": "m3", "code": "subscription_paused"})),
        (
            402,
            json!({"message": "m4", "code": "subscription_required"}),
        ),
        (422, json!({"message": "m5", "code": "plan_limit"})),
        (429, json!({"message": "slow"})),
        (500, json!({"message": "boom"})),
    ];
    let server = MockServer::start();
    let c = client(&server).with_token("t");
    for (i, (status, body)) in cases.iter().enumerate() {
        let path = format!("/api/probe/{i}");
        server.mock(|when, then| {
            when.method(GET).path(path.as_str());
            then.status(*status).json_body(body.clone());
        });
    }
    let got: Vec<AccountError> = (0..cases.len())
        .map(|i| c.get_json(&format!("probe/{i}")).unwrap_err())
        .collect();
    assert!(matches!(&got[0], AccountError::Locked(LockReason::Suspended, m) if m == "m1"));
    assert!(matches!(&got[1], AccountError::Locked(LockReason::NotEnabled, m) if m == "m2"));
    assert!(matches!(&got[2], AccountError::Locked(LockReason::Paused, m) if m == "m3"));
    assert!(matches!(&got[3], AccountError::ReadOnly(m) if m == "m4"));
    assert!(matches!(&got[4], AccountError::PlanLimit(m) if m == "m5"));
    assert!(matches!(&got[5], AccountError::RateLimited));
    assert!(matches!(&got[6], AccountError::Server(500, m) if m == "boom"));
}

#[test]
fn unreachable_server_is_a_network_error() {
    let dead = Client::new(
        Instance::Custom(url::Url::parse("http://127.0.0.1:1/api").unwrap()),
        "1",
    );
    assert!(matches!(
        auth::me(&dead).unwrap_err(),
        AccountError::Network(_)
    ));
}
