use lf_fixtures::case_json;
use lf_settings::groups::{DEFAULT_COPY_GROUPS, GroupKey, apply_groups, groups_in, pick_groups};
use lf_settings::looks::{LOOKS, apply_look};
use lf_settings::{Crop, EditSettings, Spot, SpotMode};

#[test]
fn every_built_in_look_matches_its_golden_case() {
    for look in LOOKS.iter() {
        let want = case_json(&format!("look-{}", look.id)).unwrap();
        let got = apply_look(&EditSettings::default_settings(), &look.settings).to_value();
        assert_eq!(got, want, "look-{}", look.id);
    }
}

#[test]
fn a_look_keeps_the_photo_s_crop_and_spots() {
    let mut s = EditSettings::default_settings();
    s.crop = Crop {
        x: 0.1,
        y: 0.1,
        w: 0.5,
        h: 0.5,
    };
    s.spots.push(Spot {
        x: 0.5,
        y: 0.5,
        sx: 0.6,
        sy: 0.6,
        r: 0.02,
        mode: SpotMode::Heal,
    });
    let out = apply_look(&s, &LOOKS[0].settings);
    assert_eq!(out.crop, s.crop);
    assert_eq!(out.spots, s.spots);
    assert_eq!(out.contrast, 8.0);
}

#[test]
fn looks_do_not_pile_up() {
    let a = apply_look(&EditSettings::default_settings(), &LOOKS[4].settings);
    let b = apply_look(&a, &LOOKS[9].settings);
    assert_eq!(b.temperature, 0.0);
    assert!(b.bw);
}

#[test]
fn pick_and_apply_groups_round_trip_and_default_groups_skip_geometry() {
    let mut s = EditSettings::default_settings();
    s.exposure = 1.5;
    s.angle = 10.0;
    let picked = pick_groups(&s, &DEFAULT_COPY_GROUPS);
    assert!(picked.get("angle").is_none());
    assert_eq!(picked["exposure"], 1.5);
    let onto = EditSettings::default_settings();
    assert_eq!(
        apply_groups(&onto, &picked, &DEFAULT_COPY_GROUPS).exposure,
        1.5
    );
    assert_eq!(groups_in(&picked), DEFAULT_COPY_GROUPS.to_vec());
    assert_eq!(
        groups_in(&pick_groups(&s, &[GroupKey::Light])),
        vec![GroupKey::Light]
    );
}
