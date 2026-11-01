//! Mapping a read Lightroom preset to our partial settings JSON and notes.

use lf_settings::calibration::Calibration;
use lf_settings::defringe::Defringe;
use lf_settings::groups::{GroupKey, groups_in};
use lf_settings::lens::Lens;
use lf_settings::lightroom::map::{self, CROP_NOTE, KELVIN_NOTE, UNMAPPED_NOTE};
use lf_settings::lightroom::{LrKind, LrPreset, is_lightroom};
use lf_settings::point_colour::Points;
use lf_settings::shader_values::kelvin_wb;
use lf_settings::{HslBand, normalise};
use serde_json::{Map, Value, json};

fn preset(settings: Value) -> LrPreset {
    let Value::Object(settings) = settings else {
        panic!("settings must be an object");
    };
    LrPreset {
        name: "Warm Fade".into(),
        group: Some("Studio".into()),
        kind: LrKind::Preset,
        process_version: Some("11.0".into()),
        settings,
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn light_and_colour_map_value_for_value() {
    let m = map::map(&preset(json!({
        "Exposure2012": 0.5, "Contrast2012": 20, "Highlights2012": -30, "Shadows2012": 40,
        "Whites2012": 10, "Blacks2012": -15,
        "IncrementalTemperature": 12, "IncrementalTint": -140,
        "Vibrance": 25, "Saturation": -10,
        "Texture": 5, "Clarity2012": 15, "Dehaze": 8,
        "HueAdjustmentOrange": -12, "SaturationAdjustmentBlue": 30, "LuminanceAdjustmentAqua": -5,
        "Sharpness": 40, "SharpenRadius": 1.2, "LuminanceSmoothing": 10, "ColorNoiseReduction": 25,
        "PostCropVignetteAmount": -20, "PostCropVignetteMidpoint": 40,
        "GrainAmount": 15, "GrainSize": 30,
        "ConvertToGrayscale": false
    })));
    let s = normalise(&m.preset);
    assert_eq!(m.preset["v"], json!(2));
    assert_eq!(s.exposure, 0.5);
    assert_eq!(
        (s.contrast, s.highlights, s.shadows, s.whites, s.blacks),
        (20.0, -30.0, 40.0, 10.0, -15.0)
    );
    assert_eq!((s.temperature, s.tint), (12.0, -100.0), "tint clamps");
    assert_eq!((s.vibrance, s.saturation), (25.0, -10.0));
    assert_eq!((s.texture, s.clarity, s.dehaze), (5.0, 15.0, 8.0));
    assert_eq!(s.hsl[HslBand::Orange].h, -12.0);
    assert_eq!(s.hsl[HslBand::Blue].s, 30.0);
    assert_eq!(s.hsl[HslBand::Aqua].l, -5.0);
    assert_eq!(
        (s.sharpen, s.sharpen_radius, s.noise, s.color_noise),
        (40.0, 1.2, 10.0, 25.0)
    );
    assert_eq!((s.vignette, s.vignette_midpoint), (-20.0, 40.0));
    assert_eq!((s.grain, s.grain_size), (15.0, 30.0));
    assert!(!s.bw);
    assert!(is_lightroom(&m.preset));
    let lr = &m.preset["desktop"]["lightroom"];
    assert_eq!(lr["name"], json!("Warm Fade"));
    assert_eq!(lr["group"], json!("Studio"));
    assert_eq!(lr["processVersion"], json!("11.0"));
    assert_eq!(lr["auto"], json!({"wb": false, "tone": false}));
    assert!(m.notes.is_empty(), "{:?}", m.notes);
}

#[test]
fn absent_groups_are_not_carried() {
    let m = map::map(&preset(json!({"Exposure2012": 1.25})));
    assert_eq!(groups_in(&m.preset), vec![GroupKey::Light]);
    assert_eq!(m.preset["exposure"], json!(1.25));
    assert!(
        m.preset.get("contrast").is_none(),
        "only the keys Lightroom set are written"
    );
    assert!(m.preset.get("temperature").is_none());
    assert!(m.preset.get("crop").is_none());
}

#[test]
fn as_shot_wb_is_left_out_and_auto_is_flagged() {
    let shot = map::map(&preset(json!({"WhiteBalance": "As Shot", "Vibrance": 10})));
    assert!(shot.preset.get("temperature").is_none());
    assert!(shot.preset.get("tint").is_none());
    assert_eq!(shot.preset["vibrance"], json!(10));
    assert!(shot.preset.get("saturation").is_none());
    assert_eq!(
        shot.preset["desktop"]["lightroom"]["auto"]["wb"],
        json!(false)
    );

    let auto = map::map(&preset(json!({"WhiteBalance": "Auto", "AutoTone": true})));
    assert!(auto.preset.get("temperature").is_none());
    assert_eq!(
        auto.preset["desktop"]["lightroom"]["auto"],
        json!({"wb": true, "tone": true})
    );

    let kelvin = map::map(&preset(json!({
        "WhiteBalance": "Custom", "Temperature": 3000, "Tint": 30
    })));
    let s = normalise(&kelvin.preset);
    let (t, n) = kelvin_wb(5500.0, 3000.0).unwrap();
    assert!(close(s.temperature, t), "{} vs {t}", s.temperature);
    assert!(
        close(s.tint, (n + 20.0).clamp(-100.0, 100.0)),
        "{} vs {n} + 20",
        s.tint
    );
    assert!(s.temperature < 0.0, "tungsten light cools the photo");
    assert!(kelvin.notes.iter().any(|n| n == KELVIN_NOTE));
}

#[test]
fn a_kelvin_white_balance_matches_our_presets() {
    let cloudy = map::map(&preset(json!({
        "WhiteBalance": "Cloudy", "Temperature": 6500, "Tint": 0
    })));
    let (t, n) = kelvin_wb(5500.0, 6500.0).unwrap();
    let s = normalise(&cloudy.preset);
    assert!(close(s.temperature, t) && close(s.tint, n), "{s:?}");
    assert!(s.temperature > 0.0, "cloudy warms");
}

#[test]
fn tone_curve_points_scale_to_0_1() {
    let m = map::map(&preset(json!({
        "ToneCurvePV2012": ["0, 0", "64, 50", "255, 255"],
        "ToneCurvePV2012Red": ["0, 10", "255, 245"]
    })));
    let s = normalise(&m.preset);
    assert_eq!(s.curve.rgb[1], [64.0 / 255.0, 50.0 / 255.0]);
    assert_eq!(s.curve.r, vec![[0.0, 10.0 / 255.0], [1.0, 245.0 / 255.0]]);
    assert_eq!(s.curve.g, vec![[0.0, 0.0], [1.0, 1.0]]);
    assert_eq!(groups_in(&m.preset), vec![GroupKey::Curve]);
}

#[test]
fn a_zero_parametric_curve_keeps_the_point_curve() {
    let points: Vec<String> = (0..12)
        .map(|i| format!("{}, {}", i * 23, (i * 23 + 5).min(255)))
        .collect();
    let m = map::map(&preset(json!({
        "ToneCurvePV2012": points,
        "ParametricShadows": 0, "ParametricDarks": 0, "ParametricLights": 0,
        "ParametricHighlights": 0, "ParametricShadowSplit": 25,
        "ParametricMidtoneSplit": 50, "ParametricHighlightSplit": 75
    })));
    let s = normalise(&m.preset);
    assert_eq!(s.curve.rgb.len(), 12);
    assert_eq!(s.curve.rgb[3], [69.0 / 255.0, 74.0 / 255.0]);
}

#[test]
fn a_parametric_curve_is_sampled_finely() {
    let m = map::map(&preset(json!({"ParametricLights": 40})));
    assert_eq!(normalise(&m.preset).curve.rgb.len(), 16);
}

#[test]
fn parametric_curve_lifts_shadows() {
    let m = map::map(&preset(json!({"ParametricShadows": 50})));
    let s = normalise(&m.preset);
    let table = lf_settings::curve::curve_table(&s.curve.rgb, 1001);
    assert!(table[150] > 0.15 + 0.05, "{}", table[150]);
    assert!(table[0].abs() < 1e-6 && (table[1000] - 1.0).abs() < 1e-6);
    // The highlights barely move.
    assert!((table[950] - 0.95).abs() < 0.01, "{}", table[950]);
}

#[test]
fn split_toning_becomes_grading() {
    let m = map::map(&preset(json!({
        "SplitToningShadowHue": 200, "SplitToningShadowSaturation": 20,
        "SplitToningHighlightHue": 40, "SplitToningHighlightSaturation": 30,
        "SplitToningBalance": -25
    })));
    let s = normalise(&m.preset);
    assert_eq!(
        (s.grading.shadows.hue, s.grading.shadows.sat),
        (200.0, 20.0)
    );
    assert_eq!(
        (s.grading.highlights.hue, s.grading.highlights.sat),
        (40.0, 30.0)
    );
    // Lightroom's negative balance favours the shadows; ours does that above 0.
    assert_eq!(s.grading.balance, 25.0);
    assert_eq!(s.grading.midtones.sat, 0.0);

    // Lightroom 10+ keeps shadows and highlights in the split toning keys and adds midtones.
    let wheels = map::map(&preset(json!({
        "SplitToningShadowHue": 200, "SplitToningShadowSaturation": 20,
        "ColorGradeMidtoneHue": 30, "ColorGradeMidtoneSat": 12,
        "ColorGradeBlending": 50, "ColorGradeMidtoneLum": 0
    })));
    let s = normalise(&wheels.preset);
    assert_eq!(
        (s.grading.shadows.hue, s.grading.shadows.sat),
        (200.0, 20.0)
    );
    assert_eq!(
        (s.grading.midtones.hue, s.grading.midtones.sat),
        (30.0, 12.0)
    );
    assert!(
        wheels.notes.is_empty(),
        "defaults are not noted: {:?}",
        wheels.notes
    );
}

#[test]
fn calibration_and_lens_land_in_desktop() {
    let m = map::map(&preset(json!({
        "RedHue": 10, "RedSaturation": -20, "BlueSaturation": 30, "ShadowTint": 5,
        "LensManualDistortionAmount": 12, "VignetteAmount": 40,
        "PerspectiveVertical": -15, "PerspectiveHorizontal": 7,
        "ChromaticAberrationR": 10, "ChromaticAberrationB": -5,
        "DefringePurpleAmount": 6, "DefringePurpleHueLo": 30, "DefringePurpleHueHi": 70,
        "DefringeGreenAmount": 0, "DefringeGreenHueLo": 40, "DefringeGreenHueHi": 60,
        "LensProfileEnable": 1,
        "PointColors": [
            "4.398940, 0.456522, 0.474510, 0.200000, -0.300000, 0.100000, 0.500000, 0.0, 0.1, 0.2, 0.4"
        ]
    })));
    let s = normalise(&m.preset);
    assert_eq!(
        Calibration::from_settings(&s),
        Calibration {
            red_hue: 10.0,
            red_saturation: -20.0,
            blue_saturation: 30.0,
            shadows_tint: 5.0,
            ..Calibration::default()
        }
    );
    assert_eq!(
        Lens::from_settings(&s),
        Lens {
            distortion: 12.0,
            vignetting: 40.0,
            vertical: 15.0,
            ca_red: 10.0,
            ca_blue: -5.0,
            horizontal: 7.0,
        }
    );
    let d = Defringe::from_settings(&s);
    assert_eq!(d.purple_amount, 6.0);
    assert_eq!(d.green_amount, 0.0);
    assert!(
        d.purple_hue > 240.0 && d.purple_hue < 330.0,
        "{}",
        d.purple_hue
    );
    let points = Points::from_settings(&s);
    assert_eq!(points.0.len(), 1);
    let p = points.0[0];
    assert!(
        (p.hue - 252.04).abs() < 0.1,
        "radians to degrees: {}",
        p.hue
    );
    assert!(close(p.saturation, 0.456522) && close(p.brightness, 0.474510));
    assert!(close(p.hue_shift, 20.0) && close(p.saturation_shift, -30.0));
    assert!(close(p.luminance_shift, 10.0) && close(p.range, 50.0));
    let groups = groups_in(&m.preset);
    for g in [GroupKey::Calibration, GroupKey::Lens, GroupKey::PointColour] {
        assert!(groups.contains(&g), "{g:?} in {groups:?}");
    }
    assert!(m.notes.iter().any(|n| n == map::LENS_PROFILE_NOTE));
    assert!(m.notes.iter().any(|n| n == map::POINT_COLOUR_RANGE_NOTE));
}

#[test]
fn every_crs_key_is_kept_and_unmapped_ones_are_noted() {
    let settings = json!({
        "Exposure2012": 0.3, "FancyNewSlider": 4, "AnotherOne": "x",
        "SharpenDetail": 25, "GrainFrequency": 70, "CameraProfile": "Adobe Portrait",
        "Version": "15.0"
    });
    let m = map::map(&preset(settings.clone()));
    let kept: &Map<String, Value> = m.preset["desktop"]["lightroom"]["settings"]
        .as_object()
        .unwrap();
    assert_eq!(&Value::Object(kept.clone()), &settings);
    let unmapped: Vec<&String> = m
        .notes
        .iter()
        .filter(|n| n.starts_with(UNMAPPED_NOTE))
        .collect();
    assert_eq!(unmapped.len(), 1, "{:?}", m.notes);
    assert!(
        unmapped[0].contains("AnotherOne, FancyNewSlider"),
        "{}",
        unmapped[0]
    );
    assert!(!unmapped[0].contains("Version"));
    assert!(m.notes.iter().any(|n| n == map::GRAIN_FREQUENCY_NOTE));
    assert!(
        !m.notes.iter().any(|n| n == map::SHARPEN_EXTRA_NOTE),
        "Lightroom's default detail is not noted"
    );
    assert!(m.notes.iter().any(|n| n.contains("Adobe Portrait")));
}

#[test]
fn a_crop_is_never_carried() {
    let m = map::map(&preset(json!({
        "HasCrop": true, "CropTop": 0.1, "CropLeft": 0.05, "CropBottom": 0.9, "CropRight": 0.95,
        "CropAngle": 2.5, "Contrast2012": 10
    })));
    assert!(m.preset.get("crop").is_none());
    assert!(m.preset.get("angle").is_none());
    assert!(!groups_in(&m.preset).contains(&GroupKey::Geometry));
    assert!(m.notes.iter().any(|n| n == CROP_NOTE));
    assert!(!m.notes.iter().any(|n| n.starts_with(UNMAPPED_NOTE)));
}

#[test]
fn point_colour_ranges_at_lightroom_defaults_are_not_noted() {
    let default = "1.0, 0.5, 0.5, 0.1, 0.0, 0.0, 0.5,                    0.0, 0.166667, 0.583333, 1.0,                    0.0, 0.166667, 0.583333, 1.0,                    0.0, 0.166667, 0.583333, 1.0";
    let m = map::map(&preset(json!({"PointColors": [default]})));
    assert!(m.notes.is_empty(), "{:?}", m.notes);
    let custom = default.replacen("0.583333", "0.9", 1);
    let m = map::map(&preset(json!({"PointColors": [custom]})));
    assert!(m.notes.iter().any(|n| n == map::POINT_COLOUR_RANGE_NOTE));
}

#[test]
fn transform_only_still_leaves_distortion_neutral() {
    let m = map::map(&preset(json!({"PerspectiveVertical": 20})));
    let lens = Lens::from_settings(&normalise(&m.preset));
    assert_eq!((lens.vertical, lens.distortion), (-20.0, 0.0));
}
