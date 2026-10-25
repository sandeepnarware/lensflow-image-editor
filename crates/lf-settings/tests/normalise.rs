use lf_fixtures::{Manifest, case_json};
use lf_settings::{EditSettings, HslAdjust, HslBand, Rotate, SpotMode, normalise};
use serde_json::json;

#[test]
fn every_fixture_case_is_already_normal_and_round_trips_unchanged() {
    for id in Manifest::load().unwrap().cases {
        let v = case_json(&id).unwrap();
        assert_eq!(normalise(&v).to_value(), v, "{id}");
    }
}

#[test]
fn identity_case_equals_the_defaults() {
    assert_eq!(
        normalise(&case_json("identity").unwrap()),
        EditSettings::default_settings()
    );
}

#[test]
fn clamps_and_drops_like_the_web() {
    let s = normalise(
        &json!({ "exposure": 12, "contrast": -500, "hsl": { "red": { "h": 300 } }, "rotate": 45, "spots": [{ "x": 2, "r": 9 }], "junk": 1 }),
    );
    assert_eq!(s.exposure, 5.0);
    assert_eq!(s.contrast, -100.0);
    assert_eq!(
        s.hsl[HslBand::Red],
        HslAdjust {
            h: 100.0,
            s: 0.0,
            l: 0.0
        }
    );
    assert_eq!(s.rotate, Rotate::R0);
    assert_eq!(
        (s.spots[0].x, s.spots[0].r, s.spots[0].mode),
        (1.0, 0.25, SpotMode::Heal)
    );
    assert!(s.to_value().get("junk").is_none());
}

#[test]
fn missing_or_broken_input_is_an_unedited_photo() {
    assert_eq!(normalise(&json!(null)), EditSettings::default_settings());
    assert_eq!(normalise(&json!("x")), EditSettings::default_settings());
    assert!(normalise(&json!({})).is_default());
}

#[test]
fn desktop_object_rules() {
    let d = json!({ "develop": { "profile": "camera" }, "masks": [{ "kind": "sky" }] });
    assert_eq!(normalise(&json!({ "desktop": d })).to_value()["desktop"], d);
    assert!(normalise(&json!({ "desktop": {} })).desktop.is_none());
    assert!(normalise(&json!({ "desktop": [1, 2] })).desktop.is_none());
    assert!(normalise(&json!({ "desktop": "x" })).desktop.is_none());
    assert!(
        normalise(&json!({ "desktop": { "blob": "x".repeat(65536) } }))
            .desktop
            .is_none()
    );
    assert!(
        normalise(&json!({ "desktop": { "blob": "€".repeat(21900) } }))
            .desktop
            .is_none()
    ); // 65 700 bytes, 21 900 chars
}
