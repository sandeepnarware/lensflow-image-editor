//! Assisted culling: how sharp a photo is, how many faces it has, how many of them have their
//! eyes closed, and how sharp the main face's eyes are. The library keeps these as scores and
//! says which photos look like rejects; it never rejects one by itself.
//!
//! Sharpness is plain image processing, so it needs no model. A picture's detail is split
//! into two bands of a Gaussian pyramid: the finest (the picture less a light blur) and the
//! next (that blur less a stronger one). A sharp picture has as much in its finest band as
//! in the next; a blurred one (missed focus, camera shake) has lost the finest band first, so
//! the ratio of the two bands says how sharp it is whatever the subject's contrast. The
//! ratio is taken tile by tile and the sharpest tiles count, so a sharp subject in front of a
//! soft background is a sharp photo.
//!
//! Faces are YuNet's ([`crate::faces`]); each eye (from its landmarks) is cut out and asked
//! of Intel's open-closed-eye-0001 (a 32 × 32 classifier, Apache-2.0).

use crate::assets::{self, Asset};
use crate::faces::Face;
use crate::{AiError, Engine, Rgb};

pub const CULL_MODELS: [Asset; 2] = [assets::FACES, assets::EYES];

/// The long edge photos are scored at: large enough to see focus on a face, small enough to
/// score hundreds of photos in minutes.
pub const SCORE_EDGE: u32 = 2048;

/// The tile edge sharpness is measured in, at [`SCORE_EDGE`].
const TILE: usize = 64;
/// A tile with less detail than this (mean of the coarse band, 0..1 luma) is flat (sky, wall,
/// out-of-focus background) and says nothing about focus.
const FLAT: f32 = 0.004;
/// The band ratio of a crisp picture: a ratio this high scores 1.
const CRISP: f32 = 0.8;
/// The band ratio every smooth picture tends to however blurred (the two bands' blurs
/// differ by a fixed factor): this low scores 0.
const SMOOTH: f32 = 0.28;
/// The smallest eye distance (pixels at the scored size) an eye can be judged at.
const MIN_EYE_DISTANCE: f32 = 14.0;
/// An eye is closed when the model is at least this sure.
const CLOSED: f32 = 0.6;

/// What scoring found.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Score {
    /// 0 (blurred) to 1 (crisp), from the sharpest part of the photo.
    pub sharpness: f32,
    pub faces: u32,
    /// Faces whose eyes are both closed (of those large enough to judge).
    pub eyes_closed: u32,
    /// How sharp the largest face's eyes are, 0..1, when a face is large enough to judge.
    pub eye_sharpness: Option<f32>,
}

/// A picture's luma (Rec. 709), `w × h`.
pub fn luma(img: &Rgb) -> Vec<f32> {
    img.data
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2])
        .collect()
}

/// A separable [1 4 6 4 1] / 16 blur, `step` pixels between taps (1, then 2 for the next
/// pyramid level without making it smaller); edges clamp.
fn blur(src: &[f32], w: usize, h: usize, step: usize) -> Vec<f32> {
    const K: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];
    let mut mid = vec![0.0; w * h];
    for y in 0..h {
        let row = &src[y * w..(y + 1) * w];
        for x in 0..w {
            let mut v = 0.0;
            for (k, wt) in K.iter().enumerate() {
                let dx = (k as isize - 2) * step as isize;
                let sx = (x as isize + dx).clamp(0, w as isize - 1) as usize;
                v += wt * row[sx];
            }
            mid[y * w + x] = v;
        }
    }
    let mut out = vec![0.0; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut v = 0.0;
            for (k, wt) in K.iter().enumerate() {
                let dy = (k as isize - 2) * step as isize;
                let sy = (y as isize + dy).clamp(0, h as isize - 1) as usize;
                v += wt * mid[sy * w + x];
            }
            out[y * w + x] = v;
        }
    }
    out
}

/// The two detail bands of `luma` (`w × h`): `|luma − blur1|` and `|blur1 − blur2|`.
fn bands(luma: &[f32], w: usize, h: usize) -> (Vec<f32>, Vec<f32>) {
    let b1 = blur(luma, w, h, 1);
    let b2 = blur(&b1, w, h, 2);
    let fine = luma.iter().zip(&b1).map(|(a, b)| (a - b).abs()).collect();
    let coarse = b1.iter().zip(&b2).map(|(a, b)| (a - b).abs()).collect();
    (fine, coarse)
}

/// The fine-to-coarse ratio over a rectangle of the bands, or `None` when it is flat.
fn ratio(
    fine: &[f32],
    coarse: &[f32],
    w: usize,
    rect: (usize, usize, usize, usize),
) -> Option<f32> {
    let (x0, y0, rw, rh) = rect;
    let (mut f, mut c) = (0.0f64, 0.0f64);
    for y in y0..y0 + rh {
        for x in x0..x0 + rw {
            f += f64::from(fine[y * w + x]);
            c += f64::from(coarse[y * w + x]);
        }
    }
    let n = (rw * rh).max(1) as f64;
    let c_mean = (c / n) as f32;
    (c_mean >= FLAT).then(|| (f / c.max(1e-9)) as f32)
}

/// A ratio as a 0..1 score.
fn to_score(r: f32) -> f32 {
    ((r - SMOOTH) / (CRISP - SMOOTH)).clamp(0.0, 1.0)
}

/// How sharp `luma` (`w × h`) is, 0..1: the 90th percentile of its tiles' scores (flat tiles
/// left out; a picture with no detail at all scores 0).
pub fn sharpness(luma: &[f32], w: usize, h: usize) -> f32 {
    let (fine, coarse) = bands(luma, w, h);
    let mut tiles = Vec::new();
    let mut y = 0;
    while y + TILE / 2 <= h {
        let th = TILE.min(h - y);
        let mut x = 0;
        while x + TILE / 2 <= w {
            let tw = TILE.min(w - x);
            if let Some(r) = ratio(&fine, &coarse, w, (x, y, tw, th)) {
                tiles.push(to_score(r));
            }
            x += TILE;
        }
        y += TILE;
    }
    if tiles.is_empty() {
        return 0.0;
    }
    tiles.sort_by(f32::total_cmp);
    tiles[((tiles.len() - 1) as f32 * 0.9).round() as usize]
}

/// How sharp the rectangle `rect` (x, y, w, h, clipped to the picture) of `luma` is, 0..1,
/// or `None` when it is flat or empty.
pub fn sharpness_in(luma: &[f32], w: usize, h: usize, rect: [f32; 4]) -> Option<f32> {
    let x0 = rect[0].max(0.0) as usize;
    let y0 = rect[1].max(0.0) as usize;
    let x1 = ((rect[0] + rect[2]).max(0.0) as usize).min(w);
    let y1 = ((rect[1] + rect[3]).max(0.0) as usize).min(h);
    if x1 <= x0 + 4 || y1 <= y0 + 4 {
        return None;
    }
    // The bands of just the part, with a margin so its edges blur from real neighbours.
    let m = 6;
    let (px, py) = (x0.saturating_sub(m), y0.saturating_sub(m));
    let (pw, ph) = ((x1 + m).min(w) - px, (y1 + m).min(h) - py);
    let mut part = Vec::with_capacity(pw * ph);
    for y in py..py + ph {
        part.extend_from_slice(&luma[y * w + px..y * w + px + pw]);
    }
    let (fine, coarse) = bands(&part, pw, ph);
    ratio(&fine, &coarse, pw, (x0 - px, y0 - py, x1 - x0, y1 - y0)).map(to_score)
}

/// Where an eye is cut out for the eye model: a square round `centre`, `side` wide, from the
/// distance between the eyes (the model was trained on crops of about that size).
pub fn eye_box(centre: [f32; 2], eye_distance: f32) -> [f32; 4] {
    let side = eye_distance * 0.6;
    [centre[0] - side / 2.0, centre[1] - side / 2.0, side, side]
}

/// The eye model's input for `rect` of `img`: 32 × 32, BGR, `(v·255 − 127) / 255`, planar.
pub fn eye_input(img: &Rgb, rect: [f32; 4]) -> Vec<f32> {
    const N: usize = 32;
    let mut out = vec![0.0f32; 3 * N * N];
    for y in 0..N {
        for x in 0..N {
            let sx = (rect[0] + (x as f32 + 0.5) * rect[2] / N as f32)
                .clamp(0.0, (img.w - 1) as f32) as usize;
            let sy = (rect[1] + (y as f32 + 0.5) * rect[3] / N as f32)
                .clamp(0.0, (img.h - 1) as f32) as usize;
            let [r, g, b] = img.at(sx, sy);
            let i = y * N + x;
            out[i] = (b * 255.0 - 127.0) / 255.0;
            out[N * N + i] = (g * 255.0 - 127.0) / 255.0;
            out[2 * N * N + i] = (r * 255.0 - 127.0) / 255.0;
        }
    }
    out
}

/// The distance between a face's eyes.
fn eye_distance(f: &Face) -> f32 {
    let [r, l] = [f.landmarks[0], f.landmarks[1]];
    ((r[0] - l[0]).powi(2) + (r[1] - l[1]).powi(2)).sqrt()
}

/// Where both eyes of `f` are, with a margin: the part whose sharpness is the eyes'.
pub fn eyes_rect(f: &Face) -> [f32; 4] {
    let d = eye_distance(f);
    let [r, l] = [f.landmarks[0], f.landmarks[1]];
    let (x0, x1) = (r[0].min(l[0]) - d * 0.35, r[0].max(l[0]) + d * 0.35);
    let (y0, y1) = (r[1].min(l[1]) - d * 0.25, r[1].max(l[1]) + d * 0.25);
    [x0, y0, x1 - x0, y1 - y0]
}

/// Scores photos with the face and eye models loaded once.
pub struct Scorer {
    faces: ort::session::Session,
    eyes: ort::session::Session,
}

impl Scorer {
    /// The models must be ready ([`Engine::prepare`] with [`CULL_MODELS`]).
    pub fn new(engine: &Engine) -> Result<Scorer, AiError> {
        Ok(Scorer {
            faces: engine.session(&assets::FACES)?,
            eyes: engine.session(&assets::EYES)?,
        })
    }

    /// How likely the eye in `rect` of `img` is closed, 0..1.
    fn closed(&mut self, img: &Rgb, rect: [f32; 4]) -> Result<f32, AiError> {
        let (_, out) = crate::run_one(
            &mut self.eyes,
            "input.1",
            [1, 3, 32, 32],
            eye_input(img, rect),
        )?;
        // Softmax over [open, closed].
        Ok(out.get(1).copied().unwrap_or(0.0))
    }

    /// Scores `img` (at about [`SCORE_EDGE`], as the photo was shot: no edits needed).
    pub fn score(&mut self, img: &Rgb) -> Result<Score, AiError> {
        let y = luma(img);
        let mut score = Score {
            sharpness: sharpness(&y, img.w, img.h),
            ..Score::default()
        };
        let faces = crate::faces::detect_with(&mut self.faces, img)?;
        score.faces = faces.len() as u32;
        let mut largest: Option<(f32, [f32; 4])> = None;
        for f in &faces {
            let d = eye_distance(f);
            if d < MIN_EYE_DISTANCE {
                continue;
            }
            let right = self.closed(img, eye_box(f.landmarks[0], d))?;
            let left = self.closed(img, eye_box(f.landmarks[1], d))?;
            if (right + left) / 2.0 >= CLOSED {
                score.eyes_closed += 1;
            }
            if largest.is_none_or(|(size, _)| d > size) {
                largest = Some((d, eyes_rect(f)));
            }
        }
        score.eye_sharpness = largest.and_then(|(_, rect)| sharpness_in(&y, img.w, img.h, rect));
        Ok(score)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture with detail everywhere: a fine checker under a gradient.
    fn detailed(w: usize, h: usize) -> Vec<f32> {
        let mut v = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let check = if (x / 2 + y / 2) % 2 == 0 {
                    0.25
                } else {
                    -0.25
                };
                let ring = ((x as f32 * 0.37).sin() * (y as f32 * 0.21).cos()) * 0.2;
                v.push((0.5 + check * 0.5 + ring).clamp(0.0, 1.0));
            }
        }
        v
    }

    fn gaussian(src: &[f32], w: usize, h: usize, times: usize) -> Vec<f32> {
        let mut v = src.to_vec();
        for _ in 0..times {
            v = blur(&v, w, h, 1);
        }
        v
    }

    #[test]
    fn blur_lowers_sharpness_step_by_step() {
        let (w, h) = (256, 192);
        let sharp = detailed(w, h);
        let s0 = sharpness(&sharp, w, h);
        let s1 = sharpness(&gaussian(&sharp, w, h, 2), w, h);
        let s2 = sharpness(&gaussian(&sharp, w, h, 6), w, h);
        assert!(s0 > 0.8, "crisp: {s0}");
        assert!(s1 < s0 * 0.75, "soft: {s1} vs {s0}");
        assert!(s2 < s1, "softer: {s2} vs {s1}");
        assert!(s2 < 0.3, "blurred: {s2}");
    }

    #[test]
    fn sharpness_does_not_depend_on_contrast() {
        let (w, h) = (128, 128);
        let sharp = detailed(w, h);
        let faint: Vec<f32> = sharp.iter().map(|v| 0.5 + (v - 0.5) * 0.3).collect();
        let a = sharpness(&sharp, w, h);
        let b = sharpness(&faint, w, h);
        assert!((a - b).abs() < 0.05, "{a} vs {b}");
    }

    #[test]
    fn a_flat_picture_scores_nothing_and_a_sharp_subject_is_enough() {
        let (w, h) = (256, 256);
        assert_eq!(sharpness(&vec![0.4; w * h], w, h), 0.0);
        // A sharp patch on a soft, nearly flat background.
        let mut v = gaussian(&detailed(w, h), w, h, 8);
        let crisp = detailed(w, h);
        for y in 96..224 {
            for x in 96..224 {
                v[y * w + x] = crisp[y * w + x];
            }
        }
        assert!(sharpness(&v, w, h) > 0.8);
        let inside = sharpness_in(&v, w, h, [110.0, 110.0, 80.0, 80.0]).unwrap();
        let outside = sharpness_in(&v, w, h, [4.0, 4.0, 60.0, 60.0]).unwrap_or(0.0);
        assert!(
            inside > 0.8 && outside < inside * 0.6,
            "{inside} vs {outside}"
        );
        assert_eq!(sharpness_in(&v, w, h, [300.0, 0.0, 10.0, 10.0]), None);
    }

    #[test]
    fn the_real_photos_are_crisp_until_blurred() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/sources");
        for name in ["landscape.png", "portrait.png", "mixed-light.png"] {
            let img = image::open(dir.join(name)).unwrap().to_rgba8();
            let rgb = Rgb::from_rgba8(&img);
            let y = luma(&rgb);
            let s = sharpness(&y, rgb.w, rgb.h);
            let soft = sharpness(&gaussian(&y, rgb.w, rgb.h, 4), rgb.w, rgb.h);
            assert!(s > 0.9 && soft < 0.35, "{name}: {soft} vs {s}");
        }
    }

    #[test]
    fn eyes_are_cut_out_round_their_centres() {
        assert_eq!(eye_box([100.0, 50.0], 40.0), [88.0, 38.0, 24.0, 24.0]);
        let mut img = Rgb::new(64, 64);
        // White reads (255 − 127) / 255 in every channel.
        img.data.iter_mut().for_each(|v| *v = 1.0);
        let input = eye_input(&img, [0.0, 0.0, 64.0, 64.0]);
        assert_eq!(input.len(), 3 * 32 * 32);
        assert!(input.iter().all(|v| (v - 128.0 / 255.0).abs() < 1e-6));
        let face = Face {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 100.0,
            score: 1.0,
            landmarks: [
                [30.0, 40.0],
                [70.0, 40.0],
                [50.0, 60.0],
                [35.0, 80.0],
                [65.0, 80.0],
            ],
        };
        assert_eq!(eyes_rect(&face), [16.0, 30.0, 68.0, 20.0]);
    }
}
