//! The calibration loop: rounds of `correct` until the rendered contact sheet measures like
//! the reference. The renderer is injected: here the Rust copy of the shader maths develops
//! the same picture, so a known grade must be found. Ported from `calibrate.spec.ts`.

mod support;

use lf_grade::{CalibrateOptions, Stats, calibrate, measure};
use lf_settings::{EditSettings, GradeWheel, HslAdjust, HslBand};
use support::{develop, grade, picture};

fn defaults() -> EditSettings {
    EditSettings::default_settings()
}

fn reference_of(base: &[u8], g: &EditSettings) -> Stats {
    measure(&develop(base, g), 1)
}

#[test]
fn converges_on_a_warm_bright_saturated_reference_within_four_rounds() {
    let base = picture(64);
    let anchors = measure(&base, 1).percentiles;
    let target = grade(|s| {
        s.temperature = 35.0;
        s.tint = -10.0;
        s.saturation = 25.0;
        s.curve.rgb = vec![[0.0, 0.04], [0.5, 0.6], [1.0, 0.98]];
    });
    let reference = reference_of(&base, &target);
    let mut render = |s: &EditSettings| measure(&develop(&base, s), 1);
    let result = calibrate(CalibrateOptions {
        start: &defaults(),
        reference: &reference,
        anchors: &anchors,
        render: &mut render,
        scene_bands: &[],
        rounds: None,
    });

    assert!(result.rounds <= 4);
    assert!(result.residual.luma < 0.02);
    assert!(result.residual.chroma < 0.02);
    assert!((result.settings.temperature - 35.0).abs() < 8.0);
    assert!(result.settings.saturation > 12.0);
}

#[test]
fn stops_at_once_when_the_start_already_matches() {
    let base = picture(64);
    let anchors = measure(&base, 1).percentiles;
    let g = grade(|s| s.temperature = 20.0);
    let mut render = |s: &EditSettings| measure(&develop(&base, s), 1);
    let reference = render(&g);
    let result = calibrate(CalibrateOptions {
        start: &g,
        reference: &reference,
        anchors: &anchors,
        render: &mut render,
        scene_bands: &[],
        rounds: None,
    });
    assert_eq!(result.rounds, 1);
    assert!(result.residual.luma < 0.01);
}

#[test]
fn keeps_a_black_and_white_start_and_does_not_fight_it_with_saturation() {
    let base = picture(64);
    let anchors = measure(&base, 1).percentiles;
    let bw = grade(|s| {
        s.bw = true;
        s.contrast = 15.0;
    });
    let reference = reference_of(&base, &grade(|s| s.saturation = -100.0));
    let mut render = |s: &EditSettings| measure(&develop(&base, s), 1);
    let result = calibrate(CalibrateOptions {
        start: &bw,
        reference: &reference,
        anchors: &anchors,
        render: &mut render,
        scene_bands: &[],
        rounds: None,
    });
    assert!(result.settings.bw);
    assert_eq!(result.settings.contrast, 15.0);
    assert_eq!(result.settings.saturation, 0.0);
}

#[test]
fn preserves_what_only_the_model_can_judge_vignette_grain_midtone_wheel_detail() {
    let base = picture(64);
    let anchors = measure(&base, 1).percentiles;
    let start = grade(|s| {
        s.vignette = -25.0;
        s.grain = 12.0;
        s.grain_size = 40.0;
        s.clarity = 8.0;
        s.dehaze = 4.0;
        s.grading.midtones = GradeWheel {
            hue: 30.0,
            sat: 10.0,
        };
        s.grading.balance = 15.0;
    });
    // The reference carries the same midtone tint, so white balance is the only thing left to find.
    let target = grade(|s| {
        s.temperature = 15.0;
        s.grading = start.grading;
    });
    let reference = reference_of(&base, &target);
    let mut render = |s: &EditSettings| measure(&develop(&base, s), 1);
    let result = calibrate(CalibrateOptions {
        start: &start,
        reference: &reference,
        anchors: &anchors,
        render: &mut render,
        scene_bands: &[],
        rounds: None,
    });
    let s = &result.settings;
    assert_eq!(
        (s.vignette, s.grain, s.grain_size, s.clarity, s.dehaze),
        (-25.0, 12.0, 40.0, 8.0, 4.0)
    );
    assert_eq!(
        s.grading.midtones,
        GradeWheel {
            hue: 30.0,
            sat: 10.0
        }
    );
    assert_eq!(s.grading.balance, 15.0);
    assert!(s.temperature > 5.0);
}

#[test]
fn leaves_scene_content_bands_alone() {
    let base = picture(64);
    let anchors = measure(&base, 1).percentiles;
    let target = grade(|s| {
        s.hsl[HslBand::Blue] = HslAdjust {
            h: 40.0,
            s: 0.0,
            l: 0.0,
        }
    });
    let reference = reference_of(&base, &target);
    let mut render = |s: &EditSettings| measure(&develop(&base, s), 1);
    let result = calibrate(CalibrateOptions {
        start: &defaults(),
        reference: &reference,
        anchors: &anchors,
        render: &mut render,
        scene_bands: &[HslBand::Blue],
        rounds: None,
    });
    assert_eq!(result.settings.hsl[HslBand::Blue].h, 0.0);
}
