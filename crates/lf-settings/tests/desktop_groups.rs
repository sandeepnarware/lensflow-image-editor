//! Copy, paste and presets carry the desktop-only edits (`EditSettings::desktop`), not just
//! the web editor's sliders: a demo pasted a photo's edit and only saturation came across.

use lf_settings::groups::{
    COPY_DEFAULT_GROUPS, GROUPS, GroupKey, apply_groups, drop_groups, groups_in, pick_groups,
};
use lf_settings::looks::apply_look;
use lf_settings::{EditSettings, normalise};
use serde_json::{Value, json};

fn all() -> Vec<GroupKey> {
    GROUPS.iter().map(|g| g.0).collect()
}

fn desktop(s: &EditSettings, key: &str) -> Option<Value> {
    s.desktop.as_ref().and_then(|d| d.get(key)).cloned()
}

fn edited() -> EditSettings {
    normalise(&json!({
        "v": 2,
        "saturation": 30,
        "desktop": {
            "develop": {"profile": "linear"},
            "lens": {"distortion": 10},
            "defringe": {"purple": 5},
            "calibration": {"redHue": 20},
            "pointColour": [{"h": 0.1}],
            "lut": {"id": "abc", "amount": 80},
            "negative": {"on": true},
            "masks": [{"kind": "radial"}],
            "heal": [{"r": 0.02}],
            "lensBlur": {"amount": 40},
        }
    }))
}

#[test]
fn every_group_ticked_carries_every_desktop_edit() {
    let source = edited();
    let picked = pick_groups(&source, &all());
    let pasted = apply_groups(
        &EditSettings::default_settings(),
        &picked,
        &groups_in(&picked),
    );
    assert_eq!(pasted.saturation, 30.0);
    assert_eq!(pasted.desktop, source.desktop, "{picked}");
}

#[test]
fn a_ticked_group_the_source_lacks_clears_the_target() {
    // As Lightroom: pasting "Lens corrections" from an uncorrected photo turns them off.
    let picked = pick_groups(&EditSettings::default_settings(), &[GroupKey::Lens]);
    assert_eq!(groups_in(&picked), [GroupKey::Lens], "{picked}");
    let pasted = apply_groups(&edited(), &picked, &groups_in(&picked));
    assert_eq!(desktop(&pasted, "lens"), None);
    assert_eq!(desktop(&pasted, "defringe"), None);
    assert!(
        desktop(&pasted, "calibration").is_some(),
        "other groups stay"
    );
}

#[test]
fn an_unticked_group_leaves_the_target_alone() {
    let mut onto = EditSettings::default_settings();
    onto.desktop = edited().desktop;
    let source = normalise(&json!({"v": 2, "desktop": {"calibration": {"redHue": -5}}}));
    let picked = pick_groups(&source, &[GroupKey::Calibration]);
    let pasted = apply_groups(&onto, &picked, &groups_in(&picked));
    assert_eq!(desktop(&pasted, "calibration"), Some(json!({"redHue": -5})));
    assert_eq!(desktop(&pasted, "masks"), desktop(&onto, "masks"));
}

#[test]
fn clearing_the_last_desktop_edit_drops_the_desktop_object() {
    let onto = normalise(&json!({"v": 2, "desktop": {"lens": {"distortion": 10}}}));
    let picked = pick_groups(&EditSettings::default_settings(), &[GroupKey::Lens]);
    assert_eq!(
        apply_groups(&onto, &picked, &[GroupKey::Lens]).desktop,
        None
    );
}

#[test]
fn the_copy_defaults_are_the_look_not_what_belongs_to_one_photo() {
    for g in [
        GroupKey::Lens,
        GroupKey::Calibration,
        GroupKey::Lut,
        GroupKey::PointColour,
    ] {
        assert!(COPY_DEFAULT_GROUPS.contains(&g), "{g:?}");
    }
    for g in [
        GroupKey::Geometry,
        GroupKey::Spots,
        GroupKey::Masks,
        GroupKey::LensBlur,
    ] {
        assert!(!COPY_DEFAULT_GROUPS.contains(&g), "{g:?}");
    }
}

#[test]
fn spot_removal_includes_healing_strokes_and_drops_together() {
    let picked = pick_groups(&edited(), &[GroupKey::Spots, GroupKey::Calibration]);
    assert!(picked["desktop"].get("heal").is_some(), "{picked}");
    let mut kept = picked.clone();
    drop_groups(&mut kept, &[GroupKey::Spots]);
    assert!(kept.get("spots").is_none());
    assert!(kept["desktop"].get("heal").is_none(), "{kept}");
    assert_eq!(groups_in(&kept), [GroupKey::Calibration]);
}

#[test]
fn a_preset_carrying_desktop_edits_applies_them() {
    let preset = pick_groups(&edited(), &COPY_DEFAULT_GROUPS);
    let mut onto = EditSettings::default_settings();
    onto.desktop = normalise(&json!({"v": 2, "desktop": {"masks": [{"kind": "linear"}]}})).desktop;
    let next = apply_look(&onto, &preset);
    assert_eq!(next.saturation, 30.0);
    assert_eq!(desktop(&next, "calibration"), Some(json!({"redHue": 20})));
    assert_eq!(
        desktop(&next, "lut"),
        Some(json!({"id": "abc", "amount": 80}))
    );
    assert_eq!(
        desktop(&next, "masks"),
        Some(json!([{"kind": "linear"}])),
        "the photo's own masks stay"
    );
    // A web preset (no desktop groups) still leaves desktop edits alone.
    let web = apply_look(&next, &json!({"v": 2, "contrast": 20}));
    assert_eq!(desktop(&web, "calibration"), Some(json!({"redHue": 20})));
}
