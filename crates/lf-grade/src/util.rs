use lf_settings::{EditSettings, normalise};

pub(crate) fn clamp(v: f64, min: f64, max: f64) -> f64 {
    v.max(min).min(max)
}

/// JavaScript's `Math.round`: halves go up.
pub(crate) fn js_round(v: f64) -> f64 {
    (v + 0.5).floor()
}

pub(crate) fn round1(v: f64) -> f64 {
    js_round(v * 10.0) / 10.0
}

/// A curve that is a straight line within rounding becomes the two-point identity.
pub(crate) fn identity_if_straight(points: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    if points.iter().all(|[x, y]| (y - x).abs() < 0.002) {
        vec![[0.0, 0.0], [1.0, 1.0]]
    } else {
        points
    }
}

/// Settings made safe, as the web's `normaliseSettings`.
pub fn normalise_settings(s: &EditSettings) -> EditSettings {
    normalise(&s.to_value())
}
