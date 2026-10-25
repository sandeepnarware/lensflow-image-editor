use httpmock::prelude::*;
use lf_account::grade::{self, Mode};
use lf_account::{AccountError, Client, Instance, LockReason};
use lf_settings::looks::LOOKS;
use serde_json::json;

const JPEG_REF: &[u8] = &[
    0xFF, 0xD8, 0xFF, b'R', b'E', b'F', b'B', b'Y', b'T', b'E', b'S',
];
const JPEG_SHEET: &[u8] = &[
    0xFF, 0xD8, 0xFF, b'S', b'H', b'E', b'E', b'T', b'B', b'Y', b'T', b'E', b'S',
];

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
        JPEG_REF,
        JPEG_SHEET,
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
    let p = grade::extract(
        &client(&server),
        Mode::Review,
        JPEG_REF,
        JPEG_SHEET,
        &looks,
        Some(&s),
    )
    .unwrap();
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
    let r = grade::extract(
        &client(&server),
        Mode::Extract,
        JPEG_REF,
        JPEG_SHEET,
        &[],
        None,
    );
    assert!(matches!(r, Err(AccountError::PlanLimit(ref s)) if s == "No AI requests left."));
    m.delete();
    server.mock(|when, then| {
        when.method(POST).path("/api/desktop/grade");
        then.status(402)
            .json_body(json!({"code": "subscription_paused", "message": "Paused."}));
    });
    let r = grade::extract(
        &client(&server),
        Mode::Extract,
        JPEG_REF,
        JPEG_SHEET,
        &[],
        None,
    );
    assert!(matches!(
        r,
        Err(AccountError::Locked(LockReason::Paused, _))
    ));
}

fn guard(mode: Mode, reference: &[u8], sheet: &[u8], examples: usize, settings: bool, field: &str) {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.any_request();
        then.status(200).json_body(json!({"data": {}}));
    });
    let looks: Vec<&_> = LOOKS.iter().cycle().take(examples).collect();
    let s = lf_settings::EditSettings::default_settings();
    let r = grade::extract(
        &client(&server),
        mode,
        reference,
        sheet,
        &looks,
        settings.then_some(&s),
    );
    match r {
        Err(AccountError::Validation(map)) => assert!(map.contains_key(field), "{map:?}"),
        other => panic!("{other:?}"),
    }
    m.assert_calls(0);
}

#[test]
fn guards_reject_bad_input_without_a_request() {
    let big = [vec![0xFF, 0xD8, 0xFF], vec![0; 700 * 1024]].concat();
    guard(Mode::Extract, &big, JPEG_SHEET, 0, false, "reference");
    guard(Mode::Extract, b"GIF89a", JPEG_SHEET, 0, false, "reference");
    guard(Mode::Extract, JPEG_REF, &big, 0, false, "sheet");
    guard(Mode::Extract, JPEG_REF, b"", 0, false, "sheet");
    guard(Mode::Extract, JPEG_REF, JPEG_SHEET, 17, false, "examples");
    guard(Mode::Review, JPEG_REF, JPEG_SHEET, 0, false, "settings");
}
