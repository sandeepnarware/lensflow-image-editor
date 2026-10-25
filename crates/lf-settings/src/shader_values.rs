//! Settings to the numbers the develop shader takes, ported from the web renderer's `shaderValues`.

use crate::{EditSettings, HSL_BANDS, HSL_HUES};

/// Hue (degrees) to an RGB colour at full saturation.
pub fn hue_color(hue: f64) -> [f32; 3] {
    let [r, g, b] = hue_color_f64(hue);
    [r as f32, g as f32, b as f32]
}

fn hue_color_f64(hue: f64) -> [f64; 3] {
    let h = (((hue % 360.0) + 360.0) % 360.0) / 60.0;
    let x = 1.0 - ((h % 2.0) - 1.0).abs();
    if h < 1.0 {
        [1.0, x, 0.0]
    } else if h < 2.0 {
        [x, 1.0, 0.0]
    } else if h < 3.0 {
        [0.0, 1.0, x]
    } else if h < 4.0 {
        [0.0, x, 1.0]
    } else if h < 5.0 {
        [x, 0.0, 1.0]
    } else {
        [1.0, 0.0, x]
    }
}

/// A grading wheel as an offset added to the image: the hue's colour minus its brightness,
/// so it tints without brightening.
pub fn grade_offset(hue: f64, sat: f64) -> [f32; 3] {
    let [r, g, b] = hue_color_f64(hue);
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let k = (sat / 100.0) * 0.3;
    [
        ((r - y) * k) as f32,
        ((g - y) * k) as f32,
        ((b - y) * k) as f32,
    ]
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShaderValues {
    pub wb: [f32; 3],
    pub hsl: [f32; 24],
    pub band_hue: [f32; 8],
    pub band_width: [f32; 8],
    pub grade_sh: [f32; 3],
    pub grade_mid: [f32; 3],
    pub grade_hi: [f32; 3],
}

pub fn shader_values(s: &EditSettings) -> ShaderValues {
    let t = s.temperature / 100.0;
    let tint = s.tint / 100.0;
    // Warmer: more red, less blue; tint towards magenta: less green. Normalised so white stays as bright.
    let wb = [
        1.0 + 0.32 * t + 0.06 * tint,
        1.0 - 0.22 * tint,
        1.0 - 0.32 * t + 0.06 * tint,
    ];
    let y = 0.2126 * wb[0] + 0.7152 * wb[1] + 0.0722 * wb[2];

    let gap = |a: f64, b: f64| ((a - b + 540.0) % 360.0 - 180.0).abs();
    let mut band_width = [0.0f32; 8];
    for (i, h) in HSL_HUES.iter().enumerate() {
        let prev = HSL_HUES[(i + HSL_HUES.len() - 1) % HSL_HUES.len()];
        let next = HSL_HUES[(i + 1) % HSL_HUES.len()];
        band_width[i] = gap(*h, prev).max(gap(*h, next)) as f32;
    }

    let mut hsl = [0.0f32; 24];
    for (i, b) in HSL_BANDS.iter().enumerate() {
        let a = s.hsl[*b];
        hsl[i * 3] = (a.h / 100.0) as f32;
        hsl[i * 3 + 1] = (a.s / 100.0) as f32;
        hsl[i * 3 + 2] = (a.l / 100.0) as f32;
    }

    ShaderValues {
        wb: [(wb[0] / y) as f32, (wb[1] / y) as f32, (wb[2] / y) as f32],
        hsl,
        band_hue: HSL_HUES.map(|h| h as f32),
        band_width,
        grade_sh: grade_offset(s.grading.shadows.hue, s.grading.shadows.sat),
        grade_mid: grade_offset(s.grading.midtones.hue, s.grading.midtones.sat),
        grade_hi: grade_offset(s.grading.highlights.hue, s.grading.highlights.sat),
    }
}
