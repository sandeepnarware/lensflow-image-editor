//! Pure maths for matching a reference image's grade: measuring pixel statistics, inverting
//! the shader's formulas, and one correction round. Ported from the web's `match.spec.ts`.
//! Synthetic pixel buffers stand in for photos; `support::develop` applies the same formulas
//! the shader uses, so a known change must be recovered.

mod support;

use lf_grade::{
    CorrectOptions, Correction, PERCENTILES, Stats, apply_strength, correct, exposure_offset,
    measure, normalise_settings, wheel_from_offset, white_balance_from_gains,
};
use lf_settings::shader_values::{grade_offset, shader_values};
use lf_settings::{EditSettings, GradeWheel, HslAdjust, HslBand};
use support::{develop, develop_hsl, grade, picture};

struct Ctx {
    base: Vec<u8>,
    base_stats: Stats,
    anchors: Vec<f64>,
}

fn ctx() -> Ctx {
    let base = picture(64);
    let base_stats = measure(&base, 1);
    let anchors = base_stats.percentiles.clone();
    Ctx {
        base,
        base_stats,
        anchors,
    }
}

fn defaults() -> EditSettings {
    EditSettings::default_settings()
}

/// One correction round from default settings, with the reference developed from the same picture.
fn recover(c: &Ctx, reference: &EditSettings, opts: &CorrectOptions) -> Correction {
    let r = measure(&develop(&c.base, reference), 1);
    correct(&defaults(), &c.base_stats, &r, &c.anchors, opts)
}

fn close(a: f64, b: f64, digits: i32) -> bool {
    (a - b).abs() < 10f64.powi(-digits) / 2.0
}

fn hue_gap(a: f64, b: f64) -> f64 {
    (((a - b + 540.0) % 360.0) - 180.0).abs()
}

#[test]
fn reads_percentiles_midtone_gains_saturation_zone_offsets_and_hue_bands() {
    let c = ctx();
    let s = &c.base_stats;
    assert_eq!(
        PERCENTILES,
        [1.0, 5.0, 10.0, 25.0, 50.0, 75.0, 90.0, 95.0, 99.0]
    );
    assert_eq!(s.count, 64 * 64);
    assert_eq!(s.percentiles.len(), 9);
    for i in 1..9 {
        assert!(s.percentiles[i] >= s.percentiles[i - 1]);
    }
    assert!(close(s.median, s.percentiles[4], 5));
    assert!(s.saturation > 0.1);
    assert_eq!(s.bands.len(), 8);
    assert!(close(s.bands.iter().map(|b| b.share).sum::<f64>(), 1.0, 1));
    // A neutral grey image has unit gains and no chroma anywhere.
    let g = measure(&[128u8; 16 * 4], 1);
    let gains: Vec<f64> = g
        .mid_gain
        .iter()
        .map(|v| (v * 100.0).round() / 100.0)
        .collect();
    assert_eq!(gains, [1.0, 1.0, 1.0]);
    assert_eq!(g.saturation, 0.0);
}

#[test]
fn recovers_temperature_and_tint_from_white_balance_gains() {
    for temperature in [-60.0, -20.0, 0.0, 35.0, 80.0] {
        for tint in [-40.0, 0.0, 25.0] {
            let s = grade(|s| {
                s.temperature = temperature;
                s.tint = tint;
            });
            let gains = shader_values(&s).wb.map(|v| v as f64);
            let (t, n) = white_balance_from_gains(gains);
            assert!(
                (t - temperature).abs() < 0.5,
                "temperature {temperature}/{tint}"
            );
            assert!((n - tint).abs() < 0.5, "tint {temperature}/{tint}");
        }
    }
}

#[test]
fn recovers_a_grading_wheel_from_its_colour_offset() {
    for hue in [0.0, 45.0, 120.0, 220.0, 300.0] {
        for sat in [10.0, 40.0] {
            let wheel = wheel_from_offset(grade_offset(hue, sat).map(|v| v as f64));
            assert!(hue_gap(wheel.hue, hue) <= 2.0, "hue {hue}");
            assert!((wheel.sat - sat).abs() <= 1.0, "sat {sat}");
        }
    }
    assert_eq!(wheel_from_offset([0.0, 0.0, 0.0]).sat, 0.0);
}

#[test]
fn leaves_identical_images_alone() {
    let c = ctx();
    let out = correct(
        &defaults(),
        &c.base_stats,
        &c.base_stats,
        &c.anchors,
        &CorrectOptions::default(),
    );
    assert!(out.residual.luma < 1e-6);
    assert!(out.residual.chroma < 1e-6);
    assert!(out.settings.is_default());
}

#[test]
fn recovers_a_warm_white_balance() {
    let c = ctx();
    let out = recover(
        &c,
        &grade(|s| s.temperature = 40.0),
        &CorrectOptions::default(),
    );
    assert!((out.settings.temperature - 40.0).abs() < 6.0);
    assert!(out.settings.tint.abs() < 6.0);
}

#[test]
fn lifts_the_curve_for_a_brighter_reference() {
    let c = ctx();
    let reference = grade(|s| s.curve.rgb = vec![[0.0, 0.0], [0.5, 0.65], [1.0, 1.0]]);
    let out = recover(&c, &reference, &CorrectOptions::default());
    let rgb = &out.settings.curve.rgb;
    let mid = rgb
        .iter()
        .find(|p| (p[0] - 0.5).abs() < 0.15)
        .expect("a point near the middle");
    assert!(mid[1] > mid[0] + 0.05);
    assert_eq!(rgb[0], [0.0, 0.0]);
    assert_eq!(rgb[rgb.len() - 1], [1.0, 1.0]);
}

#[test]
fn raises_saturation_for_a_more_saturated_reference() {
    let c = ctx();
    let out = recover(
        &c,
        &grade(|s| s.saturation = 30.0),
        &CorrectOptions::default(),
    );
    assert!(out.settings.saturation > 18.0);
    assert!(out.settings.saturation < 45.0);
}

#[test]
fn finds_blue_shadows_on_the_shadows_wheel() {
    let c = ctx();
    let reference = grade(|s| {
        s.grading.shadows = GradeWheel {
            hue: 220.0,
            sat: 30.0,
        }
    });
    let out = recover(&c, &reference, &CorrectOptions::default());
    assert!(hue_gap(out.settings.grading.shadows.hue, 220.0) <= 15.0);
    assert!(out.settings.grading.shadows.sat > 12.0);
}

#[test]
fn keeps_each_round_within_reason_when_the_reference_has_nothing_in_common_with_the_photos() {
    // A near-white reference (a document, a sky) against colourful photos: the fit must not blow out in one step.
    let c = ctx();
    let mut white = vec![250u8; 64 * 64 * 4];
    for i in (3..white.len()).step_by(4) {
        white[i] = 255;
    }
    let out = correct(
        &defaults(),
        &c.base_stats,
        &measure(&white, 1),
        &c.anchors,
        &CorrectOptions::default(),
    );
    let s = out.settings;
    for [x, y] in &s.curve.rgb {
        assert!(y - x <= 0.3 + 1e-6, "curve at {x}");
    }
    assert!(s.saturation >= -75.0);
    assert!(s.temperature.abs() <= 60.0);
    assert!(s.grading.shadows.sat <= 60.0);
}

#[test]
fn moves_only_the_band_that_shifted_and_skips_bands_flagged_as_scene_content() {
    let c = ctx();
    let mut shifted = defaults().hsl;
    shifted[HslBand::Green] = HslAdjust {
        h: 50.0,
        s: 0.0,
        l: 0.0,
    }; // +15 degrees
    let reference = measure(&develop_hsl(&c.base, &shifted), 1);
    let out = correct(
        &defaults(),
        &c.base_stats,
        &reference,
        &c.anchors,
        &CorrectOptions::default(),
    );
    assert!(out.settings.hsl[HslBand::Green].h > 5.0);
    assert_eq!(out.settings.hsl[HslBand::Red].h, 0.0);
    assert_eq!(out.settings.hsl[HslBand::Blue].h, 0.0);

    let opts = CorrectOptions {
        scene_bands: vec![HslBand::Green],
        ..CorrectOptions::default()
    };
    let scene = correct(&defaults(), &c.base_stats, &reference, &c.anchors, &opts).settings;
    assert_eq!(scene.hsl[HslBand::Green].h, 0.0);
}

#[test]
fn blends_the_grade_towards_neutral() {
    let mut g = defaults();
    g.temperature = 40.0;
    g.saturation = 20.0;
    g.curve.rgb = vec![[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]];
    g.grading.shadows = GradeWheel {
        hue: 220.0,
        sat: 30.0,
    };
    g.bw = true;
    let g = normalise_settings(&g);
    assert!(apply_strength(&g, 0.0).is_default());
    let half = apply_strength(&g, 50.0);
    assert_eq!(half.temperature, 20.0);
    assert!(half.saturation < 0.0); // bw at half strength becomes a saturation blend
    assert!(!half.bw);
    assert!(close(half.curve.rgb[1][1], 0.6, 5));
    assert_eq!(half.grading.shadows.sat, 15.0);
    assert_eq!(apply_strength(&g, 100.0), g);
}

#[test]
fn offsets_exposure_by_the_brightness_ratio_within_1_5_ev() {
    assert!(close(exposure_offset(0.5, 0.25), 1.0, 5));
    assert_eq!(exposure_offset(0.5, 0.5), 0.0);
    assert_eq!(exposure_offset(0.5, 0.01), 1.5);
    assert_eq!(exposure_offset(0.1, 0.8), -1.5);
}
