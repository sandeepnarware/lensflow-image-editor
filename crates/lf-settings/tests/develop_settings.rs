use lf_settings::{DevelopSettings, EditSettings, Highlights, Profile};
use serde_json::json;

#[test]
fn develop_settings_round_trip_and_default_to_no_desktop_key() {
    let mut s = EditSettings::default_settings();
    assert_eq!(
        DevelopSettings::from_settings(&s),
        DevelopSettings::default()
    );
    assert_eq!(
        DevelopSettings::default(),
        DevelopSettings {
            profile: Profile::Camera,
            highlights: Highlights::Reconstruct,
            dcp: None,
        }
    );

    let custom = DevelopSettings {
        profile: Profile::Linear,
        highlights: Highlights::Clip,
        dcp: None,
    };
    custom.write_into(&mut s);
    assert_eq!(
        s.to_value()["desktop"]["develop"],
        json!({ "profile": "linear", "highlights": "clip" })
    );
    // Survives a trip through JSON and the web's normalisation.
    let back = EditSettings::from_json_str(&s.to_value().to_string());
    assert_eq!(DevelopSettings::from_settings(&back), custom);

    // Back to defaults: no desktop key at all.
    DevelopSettings::default().write_into(&mut s);
    assert!(s.desktop.is_none());
    assert!(s.to_value().get("desktop").is_none());
}

#[test]
fn defaults_leave_other_desktop_keys_alone() {
    let mut s = EditSettings::default_settings();
    let mut map = serde_json::Map::new();
    map.insert("other".into(), json!(1));
    s.desktop = Some(map);
    DevelopSettings {
        profile: Profile::Linear,
        highlights: Highlights::Reconstruct,
        dcp: None,
    }
    .write_into(&mut s);
    assert_eq!(
        s.to_value()["desktop"]["develop"],
        json!({ "profile": "linear", "highlights": "reconstruct" })
    );
    DevelopSettings::default().write_into(&mut s);
    assert_eq!(s.to_value()["desktop"], json!({ "other": 1 }));
}

#[test]
fn unknown_or_partial_values_fall_back_to_defaults() {
    let s = EditSettings::from_json_str(
        &json!({ "desktop": { "develop": { "profile": "LINEAR?", "highlights": "clip" } } })
            .to_string(),
    );
    assert_eq!(
        DevelopSettings::from_settings(&s),
        DevelopSettings {
            profile: Profile::Camera,
            highlights: Highlights::Clip,
            dcp: None,
        }
    );
    let s = EditSettings::from_json_str(&json!({ "desktop": { "develop": 7 } }).to_string());
    assert_eq!(
        DevelopSettings::from_settings(&s),
        DevelopSettings::default()
    );
}

#[test]
fn a_camera_profile_is_kept_by_name_and_survives_other_changes() {
    let mut s = EditSettings::default_settings();
    DevelopSettings::set_dcp(&mut s, Some("Adobe Standard"));
    let d = DevelopSettings::from_settings(&s);
    assert_eq!(d.dcp, Some(lf_settings::dcp_id("adobe standard")));
    assert_eq!(
        DevelopSettings::dcp_name(&s).as_deref(),
        Some("Adobe Standard")
    );
    // Changing the profile curve keeps the camera profile.
    DevelopSettings {
        profile: Profile::Linear,
        ..d
    }
    .write_into(&mut s);
    assert_eq!(
        DevelopSettings::dcp_name(&s).as_deref(),
        Some("Adobe Standard")
    );
    assert_eq!(DevelopSettings::from_settings(&s).profile, Profile::Linear);
    // Back to the built-in colour and defaults: nothing stored.
    DevelopSettings::set_dcp(&mut s, None);
    DevelopSettings::default().write_into(&mut s);
    assert_eq!(s.desktop, None);
}
