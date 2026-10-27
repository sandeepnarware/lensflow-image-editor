//! White balance and tone worked out from the photo: the eyedropper ("this should be grey"),
//! Auto white balance and Auto tone. All three only set the sliders the web editor already
//! has (temperature, tint, exposure and the five tone sliders), so a photo looks the same in
//! the web editor afterwards.
//!
//! They read a [`Sample`]: the photo as the develop shader receives it, box-reduced to
//! [`EDGE`] points on the long side and kept on the CPU (the renderer's copy lives on the
//! device). It is made once, in the open job, so a click costs nothing.

use lf_raw::open::Picture;
use lf_settings::EditSettings;
use lf_settings::shader_values::{neutral_wb, wb_gains};

/// Auto white balance found nothing neutral enough to go by.
pub const NO_NEUTRAL: &str = "Nothing in this photo is neutral enough to set the white balance from. Use the eyedropper on something grey or white.";
/// The eyedropper landed on something too dark (or off the photo).
pub const TOO_DARK: &str =
    "That spot is too dark to set the white balance from. Click something lighter.";

/// The sample's long edge, in pixels.
pub const EDGE: u32 = 256;

/// The shader's own transfer: values arrive gamma-encoded (2.2), and white balance and
/// exposure work on `pow(c, 2.2)`.
fn to_linear(c: f32) -> f64 {
    f64::from(c.max(0.0)).powf(2.2)
}

fn to_gamma(c: f64) -> f64 {
    c.max(0.0).powf(1.0 / 2.2)
}

const LUMA: [f64; 3] = [0.2126, 0.7152, 0.0722];

fn luma(c: [f64; 3]) -> f64 {
    LUMA[0] * c[0] + LUMA[1] * c[1] + LUMA[2] * c[2]
}

/// The photo, small, as the develop shader sees it (gamma-encoded, unclamped for RAW).
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub width: u32,
    pub height: u32,
    /// Row-major, row 0 at the top.
    pub rgb: Vec<[f32; 3]>,
}

impl Sample {
    /// Reduces `width`×`height` pixels read by `at(x, y)` to at most [`EDGE`] on the long side
    /// (box averaging, in the encoded values the shader receives).
    fn reduce(width: u32, height: u32, at: impl Fn(u32, u32) -> [f32; 3]) -> Sample {
        let step = width.max(height).div_ceil(EDGE).max(1);
        let (w, h) = (width.div_ceil(step), height.div_ceil(step));
        let mut rgb = Vec::with_capacity((w * h) as usize);
        for by in 0..h {
            for bx in 0..w {
                let mut sum = [0.0f32; 3];
                let mut n = 0.0f32;
                for y in (by * step)..((by + 1) * step).min(height) {
                    for x in (bx * step)..((bx + 1) * step).min(width) {
                        let p = at(x, y);
                        for c in 0..3 {
                            sum[c] += p[c];
                        }
                        n += 1.0;
                    }
                }
                rgb.push(sum.map(|s| s / n.max(1.0)));
            }
        }
        Sample {
            width: w,
            height: h,
            rgb,
        }
    }

    /// The sample of an opened picture.
    pub fn of(picture: &Picture) -> Sample {
        match picture {
            Picture::EightBit(img) => Sample::reduce(img.width(), img.height(), |x, y| {
                let p = img.get_pixel(x, y).0;
                [p[0], p[1], p[2]].map(|v| f32::from(v) / 255.0)
            }),
            Picture::Float(dev) => {
                let f = &dev.frame;
                Sample::reduce(f.width, f.height, |x, y| {
                    let i = 4 * (y as usize * f.width as usize + x as usize);
                    [
                        f.data[i].to_f32(),
                        f.data[i + 1].to_f32(),
                        f.data[i + 2].to_f32(),
                    ]
                })
            }
        }
    }

    /// The average linear colour around `(u, v)` (fractions of the photo), over a square of
    /// `radius` sample pixels each way, so one noisy pixel does not decide the balance.
    pub fn linear_at(&self, (u, v): (f64, f64), radius: u32) -> Option<[f64; 3]> {
        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) || self.rgb.is_empty() {
            return None;
        }
        let cx = ((u * f64::from(self.width)) as i64).min(i64::from(self.width) - 1);
        let cy = ((v * f64::from(self.height)) as i64).min(i64::from(self.height) - 1);
        let r = i64::from(radius);
        let mut sum = [0.0; 3];
        let mut n = 0.0;
        for y in (cy - r).max(0)..=(cy + r).min(i64::from(self.height) - 1) {
            for x in (cx - r).max(0)..=(cx + r).min(i64::from(self.width) - 1) {
                let p = self.rgb[(y * i64::from(self.width) + x) as usize];
                for c in 0..3 {
                    sum[c] += to_linear(p[c]);
                }
                n += 1.0;
            }
        }
        Some(sum.map(|s| s / n))
    }

    fn linear(&self) -> impl Iterator<Item = [f64; 3]> + '_ {
        self.rgb.iter().map(|p| p.map(to_linear))
    }
}

/// The eyedropper: the temperature and tint that make the colour around `(u, v)` grey.
/// `None` off the photo or on something too dark to say anything about the light.
pub fn pick(sample: &Sample, at: (f64, f64)) -> Option<(f64, f64)> {
    sample
        .linear_at(at, 2)
        .and_then(neutral_wb)
        .map(|(t, tint)| (t.round(), tint.round()))
}

/// Auto white balance: the colour of the photo's most neutral midtones, made grey. Pixels
/// that are very dark or clipped say little about the light and are left out; of the rest,
/// the least saturated two fifths are averaged (a shades-of-grey estimate that a large
/// coloured wall does not drag as far as a plain average would).
pub fn auto_wb(sample: &Sample) -> Option<(f64, f64)> {
    let mut usable: Vec<([f64; 3], f64)> = sample
        .linear()
        .filter(|c| {
            let y = luma(*c);
            (0.01..0.85).contains(&y) && c.iter().all(|v| *v < 0.95)
        })
        .map(|c| {
            let max = c[0].max(c[1]).max(c[2]);
            let min = c[0].min(c[1]).min(c[2]);
            (c, (max - min) / max.max(1e-9))
        })
        .collect();
    if usable.len() < 16 {
        return None;
    }
    usable.sort_by(|a, b| a.1.total_cmp(&b.1));
    let keep = (usable.len() * 2 / 5).max(16);
    let mut sum = [0.0; 3];
    for (c, _) in &usable[..keep] {
        for i in 0..3 {
            sum[i] += c[i];
        }
    }
    neutral_wb(sum.map(|s| s / keep as f64)).map(|(t, tint)| (t.round(), tint.round()))
}

/// What Auto tone sets: exposure and the five tone sliders (the rest is left alone).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tone {
    pub exposure: f64,
    pub contrast: f64,
    pub highlights: f64,
    pub shadows: f64,
    pub whites: f64,
    pub blacks: f64,
}

impl Tone {
    pub fn apply(self, s: &mut EditSettings) {
        s.exposure = self.exposure;
        s.contrast = self.contrast;
        s.highlights = self.highlights;
        s.shadows = self.shadows;
        s.whites = self.whites;
        s.blacks = self.blacks;
    }
}

/// The value at fraction `q` of sorted `v`.
fn quantile(v: &[f64], q: f64) -> f64 {
    v[((v.len() - 1) as f64 * q).round() as usize]
}

/// Auto tone, under the photo's current white balance:
/// - exposure brings the log-average brightness to mid-grey (18 % linear);
/// - whites and blacks move the end points to the brightest and darkest 0.5 %, part way,
///   so nothing is pushed hard into clipping;
/// - highlights come down when much of the photo is near white, shadows come up when much
///   of it is near black;
/// - contrast is added to a flat photo and taken from a harsh one.
///
/// The shader's own formulas are inverted for whites (`wp = 1 - whites·0.2`) and blacks
/// (`bp = -blacks·0.06`); the rest are rules of thumb. Values are rounded as the sliders
/// show them.
pub fn auto_tone(sample: &Sample, settings: &EditSettings) -> Option<Tone> {
    let g = wb_gains(settings.temperature, settings.tint);
    let y = luma(g);
    let gains = g.map(|v| v / y);
    let lum: Vec<f64> = sample
        .linear()
        .map(|c| luma([c[0] * gains[0], c[1] * gains[1], c[2] * gains[2]]))
        .collect();
    if lum.len() < 16 {
        return None;
    }
    let log_avg = (lum.iter().map(|l| l.max(1e-4).ln()).sum::<f64>() / lum.len() as f64).exp();
    let exposure = ((0.18 / log_avg).log2().clamp(-3.0, 3.0) * 20.0).round() / 20.0;
    let gain = exposure.exp2();

    let mut shown: Vec<f64> = lum.iter().map(|l| to_gamma(l * gain)).collect();
    shown.sort_by(f64::total_cmp);
    let (lo, hi) = (quantile(&shown, 0.005), quantile(&shown, 0.995));
    let part = 0.7;
    let whites = ((1.0 - hi) / 0.2 * 100.0 * part).clamp(-60.0, 60.0);
    let blacks = (-lo / 0.06 * 100.0 * part).clamp(-60.0, 60.0);

    // After exposure: a bright twentieth above 0.8 brings highlights down, a dark twentieth
    // below 0.15 lifts the shadows, harder the further it goes.
    let (p05, p95) = (quantile(&shown, 0.05), quantile(&shown, 0.95));
    let highlights = -((p95 - 0.8) / 0.2 * 60.0).clamp(0.0, 60.0);
    let shadows = ((0.15 - p05) / 0.15 * 40.0).clamp(0.0, 40.0);

    let mean = shown.iter().sum::<f64>() / shown.len() as f64;
    let spread =
        (shown.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / shown.len() as f64).sqrt();
    let contrast = ((0.22 - spread) / 0.22 * 60.0).clamp(-30.0, 40.0);

    Some(Tone {
        exposure,
        contrast: contrast.round(),
        highlights: highlights.round(),
        shadows: shadows.round(),
        whites: whites.round(),
        blacks: blacks.round(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    /// A 400×300 8-bit picture made by `f(x, y)` (gamma-encoded 0..1).
    fn picture(f: impl Fn(u32, u32) -> [f32; 3]) -> Picture {
        let img = RgbaImage::from_fn(400, 300, |x, y| {
            let c = f(x, y).map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
            Rgba([c[0], c[1], c[2], 255])
        });
        Picture::EightBit(img)
    }

    /// A colour as the shader would receive it after a cast of `gains` on linear light.
    fn cast(linear: [f64; 3], gains: [f64; 3]) -> [f32; 3] {
        [0, 1, 2].map(|i| to_gamma(linear[i] * gains[i]) as f32)
    }

    #[test]
    fn the_sample_is_small_and_keeps_the_aspect() {
        let s = Sample::of(&picture(|_, _| [0.5, 0.5, 0.5]));
        assert_eq!((s.width, s.height), (200, 150));
        assert!(s.width <= EDGE && s.height <= EDGE);
        assert!((s.rgb[0][0] - 128.0 / 255.0).abs() < 1e-6);
        assert_eq!(s.linear_at((1.5, 0.5), 2), None, "off the photo");
    }

    #[test]
    fn the_eyedropper_greys_what_it_is_clicked_on() {
        // Left half: a grey card under warm light; right half: a red wall.
        let warm = wb_gains(-40.0, 10.0).map(|g| 1.0 / g);
        let s = Sample::of(&picture(|x, _| {
            if x < 200 {
                cast([0.2, 0.2, 0.2], warm)
            } else {
                [0.8, 0.1, 0.1]
            }
        }));
        let (t, tint) = pick(&s, (0.25, 0.5)).unwrap();
        // 8-bit rounding: within a slider step or two of the light's correction.
        assert!(
            (t + 40.0).abs() <= 2.0 && (tint - 10.0).abs() <= 2.0,
            "{t}, {tint}"
        );
    }

    #[test]
    fn auto_white_balance_finds_the_cast_under_a_coloured_scene() {
        // Neutral surfaces of several brightnesses plus a big saturated patch, all under one
        // cool cast.
        let cool = wb_gains(35.0, -15.0).map(|g| 1.0 / g);
        let s = Sample::of(&picture(|x, y| {
            if x > 260 {
                cast([0.05, 0.25, 0.6], cool)
            } else {
                let level = 0.05 + 0.5 * f64::from(y) / 300.0;
                cast([level; 3], cool)
            }
        }));
        let (t, tint) = auto_wb(&s).unwrap();
        assert!(
            (t - 35.0).abs() <= 3.0 && (tint + 15.0).abs() <= 3.0,
            "{t}, {tint}"
        );
    }

    #[test]
    fn auto_tone_brightens_a_dark_photo_and_darkens_a_bright_one() {
        let s = EditSettings::default_settings();
        let ramp = |lo: f32, hi: f32| {
            Sample::of(&picture(move |x, _| [lo + (hi - lo) * x as f32 / 400.0; 3]))
        };
        let dark = auto_tone(&ramp(0.02, 0.35), &s).unwrap();
        assert!(dark.exposure > 0.5, "{dark:?}");
        let bright = auto_tone(&ramp(0.6, 1.0), &s).unwrap();
        assert!(bright.exposure < 0.0, "{bright:?}");
        // Exposure lands on the slider's 0.05 steps; the rest are whole numbers in range.
        for t in [dark, bright] {
            assert!(((t.exposure * 20.0) - (t.exposure * 20.0).round()).abs() < 1e-9);
            for v in [t.contrast, t.highlights, t.shadows, t.whites, t.blacks] {
                assert_eq!(v, v.round());
                assert!((-100.0..=100.0).contains(&v));
            }
        }
    }

    #[test]
    fn auto_tone_holds_a_bright_sky_and_opens_deep_shade() {
        // A third deep shade, a third midtones, a third bright sky.
        let s = EditSettings::default_settings();
        let scene = Sample::of(&picture(|x, _| match x {
            0..134 => [0.04; 3],
            134..267 => [0.45; 3],
            _ => [0.98; 3],
        }));
        let t = auto_tone(&scene, &s).unwrap();
        assert!(t.highlights < 0.0, "sky held back: {t:?}");
        assert!(t.shadows > 0.0, "shade lifted: {t:?}");
    }

    #[test]
    fn auto_tone_stretches_a_flat_photo() {
        let s = EditSettings::default_settings();
        let flat = Sample::of(&picture(|x, _| [0.4 + 0.15 * x as f32 / 400.0; 3]));
        let t = auto_tone(&flat, &s).unwrap();
        assert!(t.contrast > 0.0, "{t:?}");
        assert!(
            t.whites > 0.0 && t.blacks < 0.0,
            "end points pulled out: {t:?}"
        );
        let mut applied = s.clone();
        t.apply(&mut applied);
        assert_eq!(applied.contrast, t.contrast);
        assert_eq!(
            applied.temperature, s.temperature,
            "white balance left alone"
        );
    }
}
