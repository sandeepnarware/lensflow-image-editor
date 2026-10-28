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
    // Warmer: more red, less blue; tint towards magenta: less green. Normalised so white stays as bright.
    let wb = wb_gains(s.temperature, s.tint);
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

/// The white-balance gains (red, green, blue) that `temperature` and `tint` (-100..100) give,
/// before they are normalised to keep white as bright. The develop shader multiplies linear
/// light by these.
pub fn wb_gains(temperature: f64, tint: f64) -> [f64; 3] {
    let t = temperature / 100.0;
    let tint = tint / 100.0;
    [
        1.0 + 0.32 * t + 0.06 * tint,
        1.0 - 0.22 * tint,
        1.0 - 0.32 * t + 0.06 * tint,
    ]
}

/// White balance presets, as Lightroom names them, and the colour temperature (kelvin) of
/// the light each is for.
pub const WB_PRESETS: [(&str, f64); 6] = [
    ("Daylight", 5500.0),
    ("Cloudy", 6500.0),
    ("Shade", 7500.0),
    ("Tungsten", 2850.0),
    ("Fluorescent", 3800.0),
    ("Flash", 5500.0),
];

/// The chromaticity of light at `kelvin`: the CIE daylight locus from 4000 K, the Planckian
/// locus (Kim et al.'s cubic fit) below.
pub fn kelvin_xy(kelvin: f64) -> (f64, f64) {
    let t = kelvin.clamp(1667.0, 25000.0);
    if t >= 4000.0 {
        let x = if t <= 7000.0 {
            -4.6070e9 / t.powi(3) + 2.9678e6 / t.powi(2) + 0.09911e3 / t + 0.244063
        } else {
            -2.0064e9 / t.powi(3) + 1.9018e6 / t.powi(2) + 0.24748e3 / t + 0.237040
        };
        (x, -3.0 * x * x + 2.87 * x - 0.275)
    } else {
        let x = -0.2661239e9 / t.powi(3) - 0.2343589e6 / t.powi(2) + 0.8776956e3 / t + 0.179910;
        let y = if t >= 2222.0 {
            -1.1063814 * x.powi(3) - 1.34811020 * x * x + 2.18555832 * x - 0.20219683
        } else {
            -0.9549476 * x.powi(3) - 1.37418593 * x * x + 2.09137015 * x - 0.16748867
        };
        (x, y)
    }
}

/// The correlated colour temperature of chromaticity `(x, y)` (McCamy), in kelvin.
pub fn kelvin_of_xy(x: f64, y: f64) -> f64 {
    let n = (x - 0.3320) / (0.1858 - y);
    (449.0 * n.powi(3) + 3525.0 * n * n + 6823.3 * n + 5520.33).clamp(1667.0, 25000.0)
}

/// Light at `kelvin` as linear sRGB, green 1.
fn light_rgb(kelvin: f64) -> [f64; 3] {
    let (x, y) = kelvin_xy(kelvin);
    let xyz = [x / y, 1.0, (1.0 - x - y) / y];
    let m = [
        [3.2406, -1.5372, -0.4986],
        [-0.9689, 1.8758, 0.0415],
        [0.0557, -0.2040, 1.0570],
    ];
    let rgb = m.map(|row| row[0] * xyz[0] + row[1] * xyz[1] + row[2] * xyz[2]);
    rgb.map(|v| v.max(1e-6) / rgb[1].max(1e-6))
}

/// The `(temperature, tint)` that balance a photo shot (and balanced) for `shot_kelvin` as
/// if for `target_kelvin`: light at the target shows neutral. Within the sliders' range, so a
/// far preset (tungsten on a daylight photo) goes as far as the sliders do.
pub fn kelvin_wb(shot_kelvin: f64, target_kelvin: f64) -> Option<(f64, f64)> {
    let shot = light_rgb(shot_kelvin);
    let target = light_rgb(target_kelvin);
    // How the target light looks in the photo as it is balanced now.
    neutral_wb([0, 1, 2].map(|i| target[i] / shot[i]))
}

/// The `(temperature, tint)` that make the linear colour `rgb` neutral grey: the inverse of
/// [`wb_gains`] (two linear equations: red and blue each equal to green after the gains).
/// Clamped to the sliders' -100..100, so a colour too far off is only made nearer neutral.
/// `None` for a colour too dark or with a channel at zero, which says nothing about the light.
pub fn neutral_wb(rgb: [f64; 3]) -> Option<(f64, f64)> {
    let [r, g, b] = rgb;
    if r.min(g).min(b) <= 1e-6 || r.max(g).max(b) < 1e-3 {
        return None;
    }
    // 0.32 r t + (0.06 r + 0.22 g) τ = g - r
    // -0.32 b t + (0.06 b + 0.22 g) τ = g - b
    let (a1, b1, c1) = (0.32 * r, 0.06 * r + 0.22 * g, g - r);
    let (a2, b2, c2) = (-0.32 * b, 0.06 * b + 0.22 * g, g - b);
    let det = a1 * b2 - a2 * b1;
    if det.abs() < 1e-12 {
        return None;
    }
    let t = (c1 * b2 - c2 * b1) / det;
    let tint = (a1 * c2 - a2 * c1) / det;
    let clamp = |v: f64| (v * 100.0).clamp(-100.0, 100.0);
    Some((clamp(t), clamp(tint)))
}

#[cfg(test)]
mod wb_tests {
    use super::*;

    #[test]
    fn kelvin_presets_warm_and_cool_the_right_way() {
        assert_eq!(
            kelvin_wb(5500.0, 5500.0).map(|(t, i)| (t.round(), i.round())),
            Some((0.0, 0.0))
        );
        let (shade, _) = kelvin_wb(5500.0, 7500.0).unwrap();
        let (tungsten, _) = kelvin_wb(5500.0, 2850.0).unwrap();
        assert!(shade > 5.0, "shade warms: {shade}");
        assert!(tungsten < -20.0, "tungsten cools: {tungsten}");
        // A photo shot under tungsten, set to tungsten: nothing to do.
        let (same, _) = kelvin_wb(2850.0, 2850.0).unwrap();
        assert!(same.abs() < 0.5);
        // The loci and McCamy agree to within a few percent.
        for k in [3000.0, 5000.0, 6500.0, 9000.0] {
            let (x, y) = kelvin_xy(k);
            assert!((kelvin_of_xy(x, y) - k).abs() / k < 0.04, "{k}");
        }
    }

    fn balanced(rgb: [f64; 3], (t, tint): (f64, f64)) -> [f64; 3] {
        let g = wb_gains(t, tint);
        [rgb[0] * g[0], rgb[1] * g[1], rgb[2] * g[2]]
    }

    #[test]
    fn the_gains_match_the_shader_values() {
        let mut s = EditSettings::default_settings();
        s.temperature = 37.0;
        s.tint = -12.0;
        let g = wb_gains(s.temperature, s.tint);
        let y = 0.2126 * g[0] + 0.7152 * g[1] + 0.0722 * g[2];
        let wb = shader_values(&s).wb;
        for i in 0..3 {
            assert!((f64::from(wb[i]) - g[i] / y).abs() < 1e-6);
        }
    }

    #[test]
    fn neutral_stays_put() {
        let (t, tint) = neutral_wb([0.3, 0.3, 0.3]).unwrap();
        assert!(t.abs() < 1e-9 && tint.abs() < 1e-9);
    }

    #[test]
    fn a_colour_cast_within_reach_becomes_grey() {
        // What a grey card looks like under a known (reachable) correction, undone.
        for want in [(30.0, 0.0), (-45.0, 20.0), (10.0, -60.0), (80.0, 40.0)] {
            let g = wb_gains(want.0, want.1);
            let card = [0.2 / g[0], 0.2 / g[1], 0.2 / g[2]];
            let got = neutral_wb(card).unwrap();
            assert!(
                (got.0 - want.0).abs() < 1e-6 && (got.1 - want.1).abs() < 1e-6,
                "{want:?} → {got:?}"
            );
            let out = balanced(card, got);
            assert!((out[0] - out[1]).abs() < 1e-9 && (out[2] - out[1]).abs() < 1e-9);
        }
    }

    #[test]
    fn warm_light_needs_a_cooler_setting() {
        // Orange-ish grey (tungsten): red high, blue low → temperature goes negative.
        let (t, _) = neutral_wb([0.4, 0.3, 0.2]).unwrap();
        assert!(t < 0.0, "{t}");
    }

    #[test]
    fn too_far_is_clamped_and_dark_is_refused() {
        let (t, tint) = neutral_wb([1.0, 0.2, 0.01]).unwrap();
        assert!((-100.0..=100.0).contains(&t) && (-100.0..=100.0).contains(&tint));
        assert_eq!(neutral_wb([0.0, 0.2, 0.2]), None);
        assert_eq!(neutral_wb([1e-5, 1e-5, 1e-5]), None);
    }
}
