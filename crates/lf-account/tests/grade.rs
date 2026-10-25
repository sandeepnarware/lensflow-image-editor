use httpmock::prelude::*;
use lf_account::grade::{self, Mode};
use lf_account::{AccountError, Client, Instance, LockReason};
use lf_settings::looks::LOOKS;
use serde_json::json;

fn client(server: &MockServer) -> Client {
    let url = url::Url::parse(&server.url("/api")).unwrap();
    Client::new(Instance::Custom(url), "1.0.0")
        .with_token("t")
        .with_studio("frames")
}

#[test]
fn extract_sends_the_multipart_parts_and_parses_the_proposal() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(POST)
            .path("/api/desktop/grade")
            .header("Authorization", "Bearer t")
            .header_includes("Content-Type", "multipart/form-data")
            .body_includes("name=\"mode\"")
            .body_includes("extract")
            .body_includes("name=\"reference\"; filename=\"reference.jpg\"")
            .body_includes("name=\"sheet\"; filename=\"sheet.jpg\"")
            .body_includes("Content-Type: image/jpeg")
            .body_includes("name=\"examples\"")
            .body_includes("REFBYTES")
            .body_includes("SHEETBYTES");
        then.status(200).json_body(json!({"data": {
            "settings": {"exposure": 0.4}, "notes": "Warm.", "scene_hues": ["blue", "bogus"],
            "confidence": {"exposure": 0.8}, "requests_used": 2}}));
    });
    let looks: Vec<&_> = LOOKS.iter().take(2).collect();
    let p = grade::extract(
        &client(&server),
        Mode::Extract,
        b"REFBYTES",
        b"SHEETBYTES",
        &looks,
        None,
    )
    .unwrap();
    m.assert();
    assert_eq!(p.settings["exposure"], json!(0.4));
    assert_eq!(p.notes, "Warm.");
    assert_eq!(p.scene_hues, vec![lf_settings::HslBand::Blue]);
    assert_eq!(p.confidence["exposure"], json!(0.8));
    assert_eq!(p.requests_used, 2);
}

#[test]
fn examples_field_is_a_json_list_of_id_name_settings() {
    let server = MockServer::start();
    let first = LOOKS[0].id;
    let m = server.mock(|when, then| {
        when.method(POST)
            .path("/api/desktop/grade")
            .body_includes(format!("[{{\"id\":\"{first}\""))
            .body_includes("name=\"settings\"");
        then.status(200)
            .json_body(json!({"data": {"settings": {}}}));
    });
    let looks: Vec<&_> = LOOKS.iter().take(1).collect();
    let s = lf_settings::EditSettings::default_settings();
    let p = grade::extract(&client(&server), Mode::Review, b"a", b"b", &looks, Some(&s)).unwrap();
    m.assert();
    assert_eq!(p.requests_used, 1);
}

#[test]
fn plan_limit_and_paused_are_typed() {
    let server = MockServer::start();
    let mut m = server.mock(|when, then| {
        when.method(POST).path("/api/desktop/grade");
        then.status(422)
            .json_body(json!({"code": "plan_limit", "message": "No AI requests left."}));
    });
    let r = grade::extract(&client(&server), Mode::Extract, b"a", b"b", &[], None);
    assert!(matches!(r, Err(AccountError::PlanLimit(ref s)) if s == "No AI requests left."));
    m.delete();
    server.mock(|when, then| {
        when.method(POST).path("/api/desktop/grade");
        then.status(402)
            .json_body(json!({"code": "subscription_paused", "message": "Paused."}));
    });
    let r = grade::extract(&client(&server), Mode::Extract, b"a", b"b", &[], None);
    assert!(matches!(
        r,
        Err(AccountError::Locked(LockReason::Paused, _))
    ));
}
