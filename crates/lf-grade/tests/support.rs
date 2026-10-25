//! Test-only: a Rust copy of the shader's colour maths (white balance, master curve,
//! saturation, grading wheels, HSL hue shift) on RGBA bytes, so the matching maths can be
//! checked against a known change without a GPU. Ported from the web's `pipeline.testutil.ts`.
#![allow(dead_code)]

use lf_grade::normalise_settings;
use lf_settings::curve::curve_table;
use lf_settings::shader_values::{grade_offset, shader_values};
use lf_settings::{EditSettings, Hsl};

pub fn luma(r: f64, g: f64, b: f64) -> f64 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

pub fn clamp01(v: f64) -> f64 {
    v.clamp(0.0, 1.0)
}

/// JavaScript's `Math.round` for a value already clamped to 0..255.
fn byte(v: f64) -> u8 {
    (v * 255.0 + 0.5).floor() as u8
}

/// The default settings with `f` applied, normalised.
pub fn grade(f: impl FnOnce(&mut EditSettings)) -> EditSettings {
    let mut s = EditSettings::default_settings();
    f(&mut s);
    normalise_settings(&s)
}

/// A varied, colourful test image: brightness left to right, hue top to bottom, n x n pixels.
pub fn picture(n: usize) -> Vec<u8> {
    let mut px = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let i = (y * n + x) * 4;
            let l = 0.08 + 0.84 * (x as f64 / (n - 1) as f64);
            let h = (y as f64 / n as f64) * 360.0;
            let s = 0.25 + (0.5 * ((x * 7 + y * 3) % 11) as f64) / 10.0;
            let c = s * (1.0 - (2.0 * l - 1.0).abs());
            let hp = h / 60.0;
            let xx = c * (1.0 - ((hp % 2.0) - 1.0).abs());
            let (r1, g1, b1) = if hp < 1.0 {
                (c, xx, 0.0)
            } else if hp < 2.0 {
                (xx, c, 0.0)
            } else if hp < 3.0 {
                (0.0, c, xx)
            } else if hp < 4.0 {
                (0.0, xx, c)
            } else if hp < 5.0 {
                (xx, 0.0, c)
            } else {
                (c, 0.0, xx)
            };
            let m = l - c / 2.0;
            px[i] = byte(clamp01(r1 + m));
            px[i + 1] = byte(clamp01(g1 + m));
            px[i + 2] = byte(clamp01(b1 + m));
            px[i + 3] = 255;
        }
    }
    px
}

/// White balance gains, master curve, saturation and grading wheels, as the shader applies them.
pub fn develop(px: &[u8], s: &EditSettings) -> Vec<u8> {
    let full = normalise_settings(s);
    let sv = shader_values(&full);
    let table = curve_table(&full.curve.rgb, 256);
    let mut out = vec![0u8; px.len()];
    let lin = |v: f64| v.powf(2.2);
    let gam = |v: f64| v.max(0.0).powf(1.0 / 2.2);
    let smooth = |e0: f64, e1: f64, v: f64| {
        let t = clamp01((v - e0) / (e1 - e0));
        t * t * (3.0 - 2.0 * t)
    };
    let off = |w: lf_settings::GradeWheel| grade_offset(w.hue, w.sat).map(|v| v as f64);
    let sh = off(full.grading.shadows);
    let mid = off(full.grading.midtones);
    let hi = off(full.grading.highlights);
    let pivot = 0.5 + (full.grading.balance / 100.0) * 0.25;
    let k = 1.0 + full.saturation / 100.0;
    let look = |v: f64| table[(clamp01(v) * 255.0 + 0.5).floor() as usize] as f64;
    for i in (0..px.len()).step_by(4) {
        let mut r = gam(lin(px[i] as f64 / 255.0) * sv.wb[0] as f64);
        let mut g = gam(lin(px[i + 1] as f64 / 255.0) * sv.wb[1] as f64);
        let mut b = gam(lin(px[i + 2] as f64 / 255.0) * sv.wb[2] as f64);
        r = look(r);
        g = look(g);
        b = look(b);
        let y = luma(r, g, b);
        r = y + (r - y) * k;
        g = y + (g - y) * k;
        b = y + (b - y) * k;
        let yg = clamp01(luma(r, g, b));
        let gs = 1.0 - smooth(0.0, pivot, yg);
        let gh = smooth(pivot, 1.0, yg);
        let gm = clamp01(1.0 - gs - gh);
        out[i] = byte(clamp01(r + sh[0] * gs + mid[0] * gm + hi[0] * gh));
        out[i + 1] = byte(clamp01(g + sh[1] * gs + mid[1] * gm + hi[1] * gh));
        out[i + 2] = byte(clamp01(b + sh[2] * gs + mid[2] * gm + hi[2] * gh));
        out[i + 3] = 255;
    }
    out
}

/// Hue shift per band on bytes, as the shader's colour mix does for `h` (100 = 30 degrees).
pub fn develop_hsl(px: &[u8], hsl: &Hsl) -> Vec<u8> {
    let mut s = EditSettings::default_settings();
    s.hsl = hsl.clone();
    let sv = shader_values(&s);
    let mut out = vec![0u8; px.len()];
    for i in (0..px.len()).step_by(4) {
        let r = px[i] as f64 / 255.0;
        let g = px[i + 1] as f64 / 255.0;
        let b = px[i + 2] as f64 / 255.0;
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let d = max - min;
        let mut h = 0.0;
        if d > 0.0 {
            h = if max == r {
                ((g - b) / d) % 6.0
            } else if max == g {
                (b - r) / d + 2.0
            } else {
                (r - g) / d + 4.0
            };
            h = ((h * 60.0) % 360.0 + 360.0) % 360.0;
        }
        let sat = if max == 0.0 { 0.0 } else { d / max };
        let mut shift = 0.0;
        for k in 0..8 {
            let dist = (((h - sv.band_hue[k] as f64 + 540.0) % 360.0) - 180.0).abs();
            let w = clamp01(1.0 - dist / sv.band_width[k] as f64);
            shift += w * w * (3.0 - 2.0 * w) * sv.hsl[k * 3] as f64 * 30.0;
        }
        let nh = (((h + shift * clamp01(sat / 0.25)) % 360.0) + 360.0) % 360.0;
        let hp = nh / 60.0;
        let xx = d * (1.0 - ((hp % 2.0) - 1.0).abs());
        let (r1, g1, b1) = if hp < 1.0 {
            (d, xx, 0.0)
        } else if hp < 2.0 {
            (xx, d, 0.0)
        } else if hp < 3.0 {
            (0.0, d, xx)
        } else if hp < 4.0 {
            (0.0, xx, d)
        } else if hp < 5.0 {
            (xx, 0.0, d)
        } else {
            (d, 0.0, xx)
        };
        out[i] = byte(clamp01(r1 + min));
        out[i + 1] = byte(clamp01(g1 + min));
        out[i + 2] = byte(clamp01(b1 + min));
        out[i + 3] = 255;
    }
    out
}
