use crate::util::{clamp, js_round};
use lf_settings::shader_values::grade_offset;
use lf_settings::{GradeWheel, HSL_BANDS, HSL_HUES};

pub const PERCENTILES: [f64; 9] = [1.0, 5.0, 10.0, 25.0, 50.0, 75.0, 90.0, 95.0, 99.0];

#[derive(Clone, Debug, PartialEq)]
pub struct BandStats {
    /// Share of the image's pixels nearest this band (0..1).
    pub share: f64,
    /// Circular mean hue in degrees, of the band's colourful pixels.
    pub hue: f64,
    /// Mean HSV saturation of the band's colourful pixels.
    pub sat: f64,
    /// Mean luma of the band's colourful pixels.
    pub lum: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stats {
    pub count: usize,
    pub median: f64,
    /// Luma at each of `PERCENTILES` (0..1).
    pub percentiles: Vec<f64>,
    /// Mean R, G, B each divided by luma over midtones (luma 0.2-0.8): white balance.
    pub mid_gain: [f64; 3],
    /// Mean HSV saturation.
    pub saturation: f64,
    /// Mean (colour - luma) in shadows (luma < 0.3) and highlights (> 0.7): split tones.
    pub shadow_offset: [f64; 3],
    pub highlight_offset: [f64; 3],
    /// In `HSL_BANDS` order.
    pub bands: Vec<BandStats>,
}

pub(crate) fn luma(r: f64, g: f64, b: f64) -> f64 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn smoothstep(e0: f64, e1: f64, v: f64) -> f64 {
    let t = clamp((v - e0) / (e1 - e0), 0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Signed angular difference a - b in (-180, 180].
pub(crate) fn hue_diff(a: f64, b: f64) -> f64 {
    ((a - b + 540.0) % 360.0) - 180.0
}

fn hsv(r: f64, g: f64, b: f64) -> (f64, f64) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let mut h = 0.0;
    if d > 1e-6 {
        h = if max == r {
            ((g - b) / d) % 6.0
        } else if max == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
        h = ((h * 60.0) % 360.0 + 360.0) % 360.0;
    }
    (h, if max > 0.0 { d / max } else { 0.0 })
}

/// The band whose centre is nearest the hue.
fn nearest_band(h: f64) -> usize {
    let mut best = 0;
    let mut best_d = 999.0;
    for (k, centre) in HSL_HUES.iter().enumerate() {
        let d = hue_diff(h, *centre).abs();
        if d < best_d {
            best_d = d;
            best = k;
        }
    }
    best
}

/// Pixel statistics of RGBA bytes, reading every `sample_every`th pixel.
pub fn measure(pixels: &[u8], sample_every: usize) -> Stats {
    let step = 4 * sample_every.max(1);
    let mut histogram = [0u32; 256];
    let mut count = 0usize;
    let (mut mid_n, mut sh_n, mut hi_n) = (0usize, 0usize, 0usize);
    let (mut mid_sum, mut sh_sum, mut hi_sum) = ([0.0; 3], [0.0; 3], [0.0; 3]);
    let mut sat_sum = 0.0;
    let mut band_n = [0.0f64; 8];
    let mut band_w = [0.0f64; 8];
    let mut band_cos = [0.0f64; 8];
    let mut band_sin = [0.0f64; 8];
    let mut band_sat = [0.0f64; 8];
    let mut band_lum = [0.0f64; 8];

    let mut i = 0;
    while i + 2 < pixels.len() {
        let r = pixels[i] as f64 / 255.0;
        let g = pixels[i + 1] as f64 / 255.0;
        let b = pixels[i + 2] as f64 / 255.0;
        let y = luma(r, g, b);
        count += 1;
        histogram[js_round(clamp(y, 0.0, 1.0) * 255.0) as usize] += 1;
        if y > 0.2 && y < 0.8 {
            mid_n += 1;
            mid_sum[0] += r / y;
            mid_sum[1] += g / y;
            mid_sum[2] += b / y;
        }
        let (h, s) = hsv(r, g, b);
        sat_sum += s;
        if y < 0.3 {
            sh_n += 1;
            sh_sum[0] += r - y;
            sh_sum[1] += g - y;
            sh_sum[2] += b - y;
        } else if y > 0.7 {
            hi_n += 1;
            hi_sum[0] += r - y;
            hi_sum[1] += g - y;
            hi_sum[2] += b - y;
        }
        let k = nearest_band(h);
        band_n[k] += 1.0;
        // Grey pixels don't say much about a hue.
        let w = smoothstep(0.0, 0.25, s);
        if w > 0.0 {
            let rad = h.to_radians();
            band_w[k] += w;
            band_cos[k] += w * rad.cos();
            band_sin[k] += w * rad.sin();
            band_sat[k] += w * s;
            band_lum[k] += w * y;
        }
        i += step;
    }

    let percentiles: Vec<f64> = PERCENTILES
        .iter()
        .map(|p| {
            let target = (p / 100.0) * count as f64;
            let mut seen = 0.0;
            for (v, n) in histogram.iter().enumerate() {
                seen += *n as f64;
                if seen >= target {
                    return v as f64 / 255.0;
                }
            }
            1.0
        })
        .collect();
    let mean = |sum: [f64; 3], n: usize, fallback: f64| {
        if n > 0 {
            let n = n as f64;
            [sum[0] / n, sum[1] / n, sum[2] / n]
        } else {
            [fallback; 3]
        }
    };
    let c = count as f64;
    Stats {
        count,
        median: percentiles[4],
        mid_gain: mean(mid_sum, mid_n, 1.0),
        saturation: if count > 0 { sat_sum / c } else { 0.0 },
        shadow_offset: mean(sh_sum, sh_n, 0.0),
        highlight_offset: mean(hi_sum, hi_n, 0.0),
        bands: (0..HSL_BANDS.len())
            .map(|k| BandStats {
                share: if count > 0 { band_n[k] / c } else { 0.0 },
                hue: if band_w[k] > 0.0 {
                    (band_sin[k].atan2(band_cos[k]).to_degrees() + 360.0) % 360.0
                } else {
                    HSL_HUES[k]
                },
                sat: if band_w[k] > 0.0 {
                    band_sat[k] / band_w[k]
                } else {
                    0.0
                },
                lum: if band_w[k] > 0.0 {
                    band_lum[k] / band_w[k]
                } else {
                    0.0
                },
            })
            .collect(),
        percentiles,
    }
}

/// Temperature and tint from white balance gains, inverting `shader_values().wb`:
/// wb is proportional to [1 + 0.32t + 0.06n, 1 - 0.22n, 1 - 0.32t + 0.06n] (t, n in -1..1), so
/// the gains' ratios give n from (R + B) / G and t from (R - B) / G.
pub fn white_balance_from_gains(gain: [f64; 3]) -> (f64, f64) {
    let [r, g, b] = gain;
    if g <= 1e-6 {
        return (0.0, 0.0);
    }
    let a = (r + b) / g;
    let n = (a - 2.0) / (0.12 + 0.22 * a);
    let t = (((r - b) / g) * (1.0 - 0.22 * n)) / 0.64;
    (
        clamp(t * 100.0, -100.0, 100.0),
        clamp(n * 100.0, -100.0, 100.0),
    )
}

/// A grading wheel whose offset is closest to the given colour offset; sat 0 for no offset.
pub fn wheel_from_offset(offset: [f64; 3]) -> GradeWheel {
    let mag = (offset[0] * offset[0] + offset[1] * offset[1] + offset[2] * offset[2]).sqrt();
    if mag < 1e-6 {
        return GradeWheel { hue: 0.0, sat: 0.0 };
    }
    let mut best_hue = 0;
    let mut best_dot = f64::NEG_INFINITY;
    let mut best_mag = 1.0;
    for hue in 0..360 {
        let d = grade_offset(hue as f64, 100.0).map(|v| v as f64);
        let dmag = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        if dmag < 1e-9 {
            continue;
        }
        let dot = (offset[0] * d[0] + offset[1] * d[1] + offset[2] * d[2]) / (mag * dmag);
        if dot > best_dot {
            best_dot = dot;
            best_hue = hue;
            best_mag = dmag;
        }
    }
    GradeWheel {
        hue: best_hue as f64,
        sat: clamp(mag / best_mag * 100.0, 0.0, 100.0),
    }
}
