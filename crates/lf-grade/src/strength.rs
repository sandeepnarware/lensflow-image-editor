use crate::util::{clamp, identity_if_straight, normalise_settings};
use lf_settings::{EditSettings, GradeWheel, HSL_BANDS, HslAdjust};

/// The grade blended towards neutral: 100 = as fitted, 0 = no change. Below 100 a
/// black-and-white grade becomes a saturation blend.
pub fn apply_strength(grade: &EditSettings, strength: f64) -> EditSettings {
    let k = clamp(strength, 0.0, 100.0) / 100.0;
    if k >= 1.0 {
        return grade.clone();
    }
    let d = EditSettings::default_settings();
    let mut out = grade.clone();
    out.exposure = grade.exposure * k;
    out.contrast = grade.contrast * k;
    out.highlights = grade.highlights * k;
    out.shadows = grade.shadows * k;
    out.whites = grade.whites * k;
    out.blacks = grade.blacks * k;
    out.temperature = grade.temperature * k;
    out.tint = grade.tint * k;
    out.vibrance = grade.vibrance * k;
    out.saturation = grade.saturation * k;
    out.clarity = grade.clarity * k;
    out.texture = grade.texture * k;
    out.dehaze = grade.dehaze * k;
    out.vignette = grade.vignette * k;
    out.grain = grade.grain * k;
    out.vignette_midpoint =
        d.vignette_midpoint + (grade.vignette_midpoint - d.vignette_midpoint) * k;
    out.grain_size = d.grain_size + (grade.grain_size - d.grain_size) * k;
    let blend = |points: &[[f64; 2]]| {
        identity_if_straight(points.iter().map(|[x, y]| [*x, x + (y - x) * k]).collect())
    };
    out.curve.rgb = blend(&grade.curve.rgb);
    out.curve.r = blend(&grade.curve.r);
    out.curve.g = blend(&grade.curve.g);
    out.curve.b = blend(&grade.curve.b);
    for band in HSL_BANDS {
        let a = grade.hsl[band];
        out.hsl[band] = HslAdjust {
            h: a.h * k,
            s: a.s * k,
            l: a.l * k,
        };
    }
    let wheel = |w: GradeWheel| GradeWheel {
        hue: w.hue,
        sat: w.sat * k,
    };
    out.grading.shadows = wheel(grade.grading.shadows);
    out.grading.midtones = wheel(grade.grading.midtones);
    out.grading.highlights = wheel(grade.grading.highlights);
    out.grading.balance = grade.grading.balance * k;
    if grade.bw {
        out.bw = false;
        out.saturation = -100.0 * k;
    }
    normalise_settings(&out)
}

/// Exposure (EV) that brings a photo's median luma to the reference's, within +-1.5 EV.
pub fn exposure_offset(reference_median: f64, photo_median: f64) -> f64 {
    clamp(
        (reference_median.max(1e-3) / photo_median.max(1e-3)).log2(),
        -1.5,
        1.5,
    )
}
