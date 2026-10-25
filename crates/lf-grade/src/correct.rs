use crate::measure::{Stats, hue_diff, wheel_from_offset, white_balance_from_gains};
use crate::util::{clamp, identity_if_straight, js_round, normalise_settings, round1};
use lf_settings::curve::curve_table;
use lf_settings::shader_values::{grade_offset, shader_values};
use lf_settings::{CurvePoint, EditSettings, GradeWheel, HSL_BANDS, HslBand};

#[derive(Clone, Debug)]
pub struct CorrectOptions {
    /// Hue bands the AI flagged as scene content in the reference: left alone.
    pub scene_bands: Vec<HslBand>,
    /// Fraction of the measured tone difference applied per round (default 0.8).
    pub curve_step: f64,
    /// Fraction applied to HSL band corrections per round (default 0.5): bands interact.
    pub hsl_step: f64,
}

impl Default for CorrectOptions {
    fn default() -> Self {
        CorrectOptions {
            scene_bands: Vec::new(),
            curve_step: 0.8,
            hsl_step: 0.5,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Residual {
    pub luma: f64,
    pub chroma: f64,
}

#[derive(Clone, Debug)]
pub struct Correction {
    pub settings: EditSettings,
    pub residual: Residual,
}

/// How far one round may move. A reference with little in common with the photos (a document,
/// a sky) must not blow the grade out in one step.
const MAX_CURVE_MOVE: f64 = 0.3;
const WB_RATIO: [f64; 2] = [0.85, 1.18];
const SAT_RATIO: [f64; 2] = [0.5, 2.0];
const MAX_WHEEL_SAT: f64 = 60.0;

/// One calibration round. `rendered` is the contact sheet measured after rendering with
/// `current`; `reference` is the reference image; `anchors` are the sheet's percentile lumas
/// measured once with default settings (the tone curve's x positions). Everything the round
/// touches is in pipeline order: white balance, master curve, saturation, grading wheels, HSL.
pub fn correct(
    current: &EditSettings,
    rendered: &Stats,
    reference: &Stats,
    anchors: &[f64],
    opts: &CorrectOptions,
) -> Correction {
    let mut next = current.clone();

    // White balance: compose the measured gain ratio onto the current gains. Gains apply in
    // linear light (the shader's pow 2.2), but the means are measured on gamma values, so
    // raise the ratio.
    let gains = shader_values(current).wb.map(|v| v as f64);
    let mut ratio = [1.0; 3];
    for (i, slot) in ratio.iter_mut().enumerate() {
        let v = rendered.mid_gain[i];
        if v > 1e-6 {
            *slot = clamp(
                (reference.mid_gain[i] / v).powf(2.2),
                WB_RATIO[0],
                WB_RATIO[1],
            );
        }
    }
    let (temperature, tint) = white_balance_from_gains([
        gains[0] * ratio[0],
        gains[1] * ratio[1],
        gains[2] * ratio[2],
    ]);
    next.temperature = round1(temperature);
    next.tint = round1(tint);

    // Master tone curve: points at the anchors, each moved towards the reference's percentile.
    let table = curve_table(&current.curve.rgb, 256);
    let mut points: Vec<CurvePoint> = Vec::new();
    for (i, &x) in anchors.iter().enumerate() {
        let y = table[js_round(clamp(x, 0.0, 1.0) * 255.0) as usize] as f64;
        let delta = reference.percentiles.get(i).copied().unwrap_or(x)
            - rendered.percentiles.get(i).copied().unwrap_or(x);
        points.push([
            x,
            clamp(
                y + clamp(opts.curve_step * delta, -MAX_CURVE_MOVE, MAX_CURVE_MOVE),
                0.0,
                1.0,
            ),
        ]);
    }
    next.curve.rgb = tidy_curve(points);

    // Saturation: the shader's saturation is multiplicative on chroma.
    let sat_ratio = clamp(
        if rendered.saturation > 1e-4 {
            reference.saturation / rendered.saturation
        } else {
            1.0
        },
        SAT_RATIO[0],
        SAT_RATIO[1],
    );
    next.saturation = round1(clamp(
        ((1.0 + current.saturation / 100.0) * sat_ratio - 1.0) * 100.0,
        -100.0,
        100.0,
    ));

    // Split tones: the wheels' offsets plus the measured difference.
    let zones = [
        (
            current.grading.shadows,
            reference.shadow_offset,
            rendered.shadow_offset,
        ),
        (
            current.grading.highlights,
            reference.highlight_offset,
            rendered.highlight_offset,
        ),
    ];
    let mut fitted_wheels = [current.grading.shadows, current.grading.highlights];
    for (z, (wheel, want, got)) in zones.iter().enumerate() {
        let base = grade_offset(wheel.hue, wheel.sat);
        let mut target = [0.0; 3];
        for i in 0..3 {
            target[i] = base[i] as f64 + want[i] - got[i];
        }
        let fitted = wheel_from_offset(target);
        // No tint to speak of: keep the wheel where it was (a hue with sat 0 is the editor's neutral).
        fitted_wheels[z] = if fitted.sat < 0.5 {
            GradeWheel {
                hue: wheel.hue,
                sat: 0.0,
            }
        } else {
            GradeWheel {
                hue: fitted.hue,
                sat: round1(fitted.sat.min(wheel.sat.max(MAX_WHEEL_SAT))),
            }
        };
    }
    next.grading.shadows = fitted_wheels[0];
    next.grading.highlights = fitted_wheels[1];

    // HSL bands: hue shift, saturation and luminance per band, relative to the global change, damped.
    for (k, band) in HSL_BANDS.iter().enumerate() {
        let band = *band;
        if opts.scene_bands.contains(&band) {
            continue;
        }
        let r = &reference.bands[k];
        let got = &rendered.bands[k];
        let weight = clamp(r.share.min(got.share) / 0.02, 0.0, 1.0);
        if weight < 0.25 {
            continue;
        }
        let cur = current.hsl[band];
        let adjust = &mut next.hsl[band];
        let dh = hue_diff(r.hue, got.hue);
        if dh.abs() >= 0.5 {
            adjust.h = round1(clamp(
                adjust.h + opts.hsl_step * weight * (dh / 30.0) * 100.0,
                -100.0,
                100.0,
            ));
        }
        if got.sat > 1e-4 && r.sat > 1e-4 {
            let band_ratio = r.sat / got.sat / sat_ratio;
            let target_s = ((1.0 + cur.s / 100.0) * band_ratio - 1.0) * 100.0;
            if (target_s - cur.s).abs() >= 0.5 {
                adjust.s = round1(clamp(
                    cur.s + opts.hsl_step * weight * (target_s - cur.s),
                    -100.0,
                    100.0,
                ));
            }
        }
        if got.lum > 1e-4 && r.lum > 1e-4 {
            let target_l =
                ((1.0 + (cur.l / 100.0) * 0.45) * (r.lum / got.lum) - 1.0) / 0.45 * 100.0;
            if (target_l - cur.l).abs() >= 0.5 {
                adjust.l = round1(clamp(
                    cur.l + opts.hsl_step * weight * (target_l - cur.l),
                    -100.0,
                    100.0,
                ));
            }
        }
    }

    let diff3 = |a: &[f64; 3], b: &[f64; 3]| -> f64 {
        (0..3).map(|i| (a[i] - b[i]).abs()).fold(0.0, f64::max)
    };
    let residual = Residual {
        luma: reference
            .percentiles
            .iter()
            .enumerate()
            .map(|(i, v)| (v - rendered.percentiles.get(i).copied().unwrap_or(*v)).abs())
            .fold(f64::NEG_INFINITY, f64::max),
        chroma: diff3(&reference.mid_gain, &rendered.mid_gain)
            .max(diff3(&reference.shadow_offset, &rendered.shadow_offset))
            .max(diff3(
                &reference.highlight_offset,
                &rendered.highlight_offset,
            )),
    };

    Correction {
        settings: normalise_settings(&next),
        residual,
    }
}

/// (0,0) and (1,1) kept, points sorted, at least 0.01 apart in x, never decreasing in y, at most 16.
pub fn tidy_curve(points: Vec<CurvePoint>) -> Vec<CurvePoint> {
    if identity_if_straight(points.clone()).len() == 2 {
        return vec![[0.0, 0.0], [1.0, 1.0]];
    }
    let mut sorted: Vec<CurvePoint> = points
        .into_iter()
        .filter(|p| p[0] > 0.005 && p[0] < 0.995)
        .collect();
    sorted.sort_by(|a, b| a[0].total_cmp(&b[0]));
    let mut out: Vec<CurvePoint> = vec![[0.0, 0.0]];
    for [x, y] in sorted {
        let last = out[out.len() - 1];
        if x - last[0] < 0.01 {
            continue;
        }
        out.push([
            js_round(x * 1000.0) / 1000.0,
            js_round(y.max(last[1]) * 1000.0) / 1000.0,
        ]);
        if out.len() >= 15 {
            break;
        }
    }
    let last = out[out.len() - 1];
    out.push([1.0, last[1].max(1.0)]);
    out
}
