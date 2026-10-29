//! High-resolution merge: a handheld burst made into one photo twice as large each way, with
//! real detail, not guessed (Photoshop's "multi-shot super resolution", the idea behind
//! pixel-shift modes done in software). A hand never holds still: each frame sees the scene
//! shifted by a fraction of a pixel, so together the frames sample it more finely than any one.
//!
//! Each frame is lined up with the middle one ([`crate::align`], then to a fraction of a pixel
//! by fitting a parabola through the match around the best whole shift), and every pixel of the
//! larger picture gathers the nearby samples of every frame, weighted by how close each lands
//! (a Gaussian, [`SIGMA`] output pixels) and by how well it agrees with the middle frame there
//! (so something that moved between the frames does not smear). Where no frame lands close,
//! the middle frame enlarged fills in. A light sharpening undoes the gathering's softness.

use rayon::prelude::*;

use crate::{MergeError, Rgb};

/// The larger picture's scale.
pub const SCALE: usize = 2;
/// How far a sample reaches, in output pixels (the Gaussian's sigma).
const SIGMA: f32 = 0.6;
/// A sample unlike the middle frame by this much (0..1) counts for little.
const AGREE: f32 = 0.08;
/// The enlarged middle frame's weight where samples are thin.
const FALLBACK: f32 = 0.05;
/// The sharpening's amount (an unsharp mask of radius 1).
const SHARPEN: f32 = 0.5;

fn grey(img: &Rgb) -> Vec<f32> {
    img.data
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2])
        .collect()
}

/// The mean squared difference of `a` and `b` moved by (`dx`, `dy`) (b's pixel at x lands on a's
/// at x + dx), over the middle of the picture, every other pixel.
fn ssd(a: &[f32], b: &[f32], w: usize, h: usize, dx: i32, dy: i32) -> f32 {
    let (mx, my) = (w / 8 + 2, h / 8 + 2);
    let (mut sum, mut n) = (0.0f64, 0usize);
    for y in (my..h - my).step_by(2) {
        let sy = y as i32 - dy;
        if sy < 0 || sy >= h as i32 {
            continue;
        }
        for x in (mx..w - mx).step_by(2) {
            let sx = x as i32 - dx;
            if sx < 0 || sx >= w as i32 {
                continue;
            }
            let d = a[y * w + x] - b[sy as usize * w + sx as usize];
            sum += f64::from(d * d);
            n += 1;
        }
    }
    (sum / n.max(1) as f64) as f32
}

/// `v` (a `w` × `h` grey picture) through a 3 × 3 box, twice.
fn soften(v: &[f32], w: usize, h: usize) -> Vec<f32> {
    let pass = |src: &[f32]| -> Vec<f32> {
        let mut out = vec![0.0; w * h];
        for y in 0..h {
            for x in 0..w {
                let (mut sum, mut n) = (0.0, 0.0);
                for yy in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                    for xx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                        sum += src[yy * w + xx];
                        n += 1.0;
                    }
                }
                out[y * w + x] = sum / n;
            }
        }
        out
    };
    pass(&pass(v))
}

/// The vertex of the parabola through (−1, `l`), (0, `c`), (1, `r`), within ±0.5.
fn vertex(l: f32, c: f32, r: f32) -> f32 {
    let curve = l - 2.0 * c + r;
    if curve <= 1e-12 {
        return 0.0;
    }
    (0.5 * (l - r) / curve).clamp(-0.5, 0.5)
}

/// The shift (in pixels, to a fraction) that lines `frame` up with `reference`: frame's pixel
/// at x shows what the reference shows at x + shift.
pub fn subpixel_shift(reference: &Rgb, frame: &Rgb) -> (f32, f32) {
    let (dx, dy) = crate::align(reference, frame, 6);
    let (w, h) = (reference.w, reference.h);
    // Softened a little: detail finer than a pixel aliases differently in each frame and
    // would pull the fit.
    let (a, b) = (soften(&grey(reference), w, h), soften(&grey(frame), w, h));
    let s = |x: i32, y: i32| ssd(&a, &b, w, h, dx + x, dy + y);
    let c = s(0, 0);
    let fx = vertex(s(-1, 0), c, s(1, 0));
    let fy = vertex(s(0, -1), c, s(0, 1));
    (dx as f32 + fx, dy as f32 + fy)
}

/// `img` bilinearly at (x, y) in pixel-centre coordinates (pixel i's centre is at i), clamped.
fn sample(img: &Rgb, x: f32, y: f32) -> [f32; 3] {
    let x = x.clamp(0.0, (img.w - 1) as f32);
    let y = y.clamp(0.0, (img.h - 1) as f32);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(img.w - 1), (y0 + 1).min(img.h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let at = |x: usize, y: usize, c: usize| img.data[(y * img.w + x) * 3 + c];
    std::array::from_fn(|c| {
        let top = at(x0, y0, c) + (at(x1, y0, c) - at(x0, y0, c)) * fx;
        let bottom = at(x0, y1, c) + (at(x1, y1, c) - at(x0, y1, c)) * fx;
        top + (bottom - top) * fy
    })
}

/// The burst `frames` (one size; the middle one is the reference) merged at [`SCALE`]×.
/// `progress(done, total)` as it goes.
pub fn merge(frames: &[Rgb], mut progress: impl FnMut(usize, usize)) -> Result<Rgb, MergeError> {
    if frames.len() < 2 {
        return Err(MergeError::TooFew);
    }
    let (w, h) = (frames[0].w, frames[0].h);
    if frames.iter().any(|f| (f.w, f.h) != (w, h)) {
        return Err(MergeError::SizesDiffer);
    }
    let total = frames.len() + 1;
    let middle = frames.len() / 2;
    let reference = &frames[middle];
    let shifts: Vec<(f32, f32)> = frames
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let s = if i == middle {
                (0.0, 0.0)
            } else {
                subpixel_shift(reference, f)
            };
            progress(i + 1, total);
            s
        })
        .collect();
    log::debug!("high resolution: shifts {shifts:?}");
    let (ow, oh) = (w * SCALE, h * SCALE);
    let k = SCALE as f32;
    let two_sigma2 = 2.0 * SIGMA * SIGMA;
    let mut out = Rgb::new(ow, oh);
    out.data
        .par_chunks_mut(ow * 3)
        .enumerate()
        .for_each(|(oy, row)| {
            for ox in 0..ow {
                // This output pixel's centre in the reference's pixel-centre coordinates.
                let (px, py) = ((ox as f32 + 0.5) / k - 0.5, (oy as f32 + 0.5) / k - 0.5);
                let base = sample(reference, px, py);
                let mut acc = base.map(|v| v * FALLBACK);
                let mut weight = FALLBACK;
                for (f, &(sx, sy)) in frames.iter().zip(&shifts) {
                    // Where this point is in the frame, and the frame's pixels about it.
                    let (fx, fy) = (px - sx, py - sy);
                    let (x0, y0) = (fx.floor() as i64, fy.floor() as i64);
                    for ny in y0..=y0 + 1 {
                        for nx in x0..=x0 + 1 {
                            if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                                continue;
                            }
                            // The sample's own place in the reference, in output pixels away.
                            let (dx, dy) = ((nx as f32 - fx) * k, (ny as f32 - fy) * k);
                            let near = (-(dx * dx + dy * dy) / two_sigma2).exp();
                            if near < 1e-3 {
                                continue;
                            }
                            let i = (ny as usize * w + nx as usize) * 3;
                            let s = [f.data[i], f.data[i + 1], f.data[i + 2]];
                            let diff = (0..3).map(|c| (s[c] - base[c]).abs()).fold(0.0, f32::max);
                            let agree = (-(diff * diff) / (2.0 * AGREE * AGREE)).exp();
                            let wgt = near * agree;
                            for c in 0..3 {
                                acc[c] += s[c] * wgt;
                            }
                            weight += wgt;
                        }
                    }
                }
                for c in 0..3 {
                    row[ox * 3 + c] = acc[c] / weight;
                }
            }
        });
    let sharpened = sharpen(&out);
    progress(total, total);
    Ok(sharpened)
}

/// `img` + [`SHARPEN`] × (`img` − its 3 × 3 box blur), kept within 0..1.
fn sharpen(img: &Rgb) -> Rgb {
    let (w, h) = (img.w, img.h);
    let mut out = Rgb::new(w, h);
    out.data
        .par_chunks_mut(w * 3)
        .enumerate()
        .for_each(|(y, row)| {
            for x in 0..w {
                for c in 0..3 {
                    let mut sum = 0.0;
                    for yy in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                        for xx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                            sum += img.data[(yy * w + xx) * 3 + c];
                        }
                    }
                    let n = ((y + 1).min(h - 1) - y.saturating_sub(1) + 1)
                        * ((x + 1).min(w - 1) - x.saturating_sub(1) + 1);
                    let v = img.data[(y * w + x) * 3 + c];
                    row[x * 3 + c] = (v + SHARPEN * (v - sum / n as f32)).clamp(0.0, 1.0);
                }
            }
        });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fine scene at twice the frames' size: stripes and dots near the frames' own limit.
    fn fine(w: usize, h: usize) -> Rgb {
        let mut img = Rgb::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = (x as f32, y as f32);
                let v = 0.5
                    + 0.25 * (fx * 0.9 + fy * 0.3).sin()
                    + 0.2 * (fy * 0.7 - fx * 0.2).cos()
                    + 0.05 * ((x * 7 + y * 13) % 5) as f32 / 4.0;
                let i = (y * w + x) * 3;
                img.data[i..i + 3].copy_from_slice(&[v.clamp(0.0, 1.0); 3]);
            }
        }
        img
    }

    /// The fine scene seen by a camera of half its resolution, its view moved by (`ox`, `oy`)
    /// fine pixels: each frame pixel averages 2 × 2 fine pixels.
    fn frame(scene: &Rgb, ox: usize, oy: usize, w: usize, h: usize) -> Rgb {
        let mut img = Rgb::new(w, h);
        for y in 0..h {
            for x in 0..w {
                for c in 0..3 {
                    let mut s = 0.0;
                    for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                        s += scene.data[((y * 2 + oy + dy) * scene.w + x * 2 + ox + dx) * 3 + c];
                    }
                    img.data[(y * w + x) * 3 + c] = s / 4.0;
                }
            }
        }
        img
    }

    fn error(a: &Rgb, b: &Rgb, ox: usize, oy: usize) -> f32 {
        // `a` is at the scene's place (ox, oy); compare away from the borders.
        let mut sum = 0.0;
        let mut n = 0;
        for y in 8..a.h - 8 {
            for x in 8..a.w - 8 {
                let d = a.data[(y * a.w + x) * 3] - b.data[((y + oy) * b.w + x + ox) * 3];
                sum += d * d;
                n += 1;
            }
        }
        (sum / n as f32).sqrt()
    }

    #[test]
    fn a_shifted_burst_merges_closer_to_the_scene_than_one_frame_enlarged() {
        let (w, h) = (90usize, 70usize);
        let scene = fine(w * 2 + 8, h * 2 + 8);
        // Eight frames at every half-pixel step (and some whole ones), the reference at (2, 2).
        let places = [
            (2, 2),
            (3, 2),
            (2, 3),
            (3, 3),
            (4, 2),
            (1, 3),
            (2, 1),
            (5, 4),
        ];
        let mut frames: Vec<Rgb> = places
            .iter()
            .map(|&(x, y)| frame(&scene, x, y, w, h))
            .collect();
        // The reference is the middle one.
        frames.swap(0, places.len() / 2);
        let reference = places[0];
        let merged = merge(&frames, |_, _| {}).unwrap();
        assert_eq!((merged.w, merged.h), (w * 2, h * 2));
        let one = Rgb {
            w: w * 2,
            h: h * 2,
            data: (0..w * 2 * h * 2)
                .flat_map(|i| {
                    let (x, y) = (i % (w * 2), i / (w * 2));
                    sample(
                        &frames[places.len() / 2],
                        (x as f32 + 0.5) / 2.0 - 0.5,
                        (y as f32 + 0.5) / 2.0 - 0.5,
                    )
                })
                .collect(),
        };
        let (got, plain) = (
            error(&merged, &scene, reference.0, reference.1),
            error(&one, &scene, reference.0, reference.1),
        );
        assert!(got < plain * 0.8, "merged {got} vs one frame {plain}");
    }

    #[test]
    fn half_a_pixel_is_found() {
        let scene = fine(208, 168);
        let a = frame(&scene, 2, 2, 100, 80);
        let b = frame(&scene, 3, 2, 100, 80);
        let (sx, sy) = subpixel_shift(&a, &b);
        // b's view is half a frame pixel to the right: its pixel x shows a's x + 0.5.
        assert!((sx - 0.5).abs() < 0.2 && sy.abs() < 0.2, "{sx}, {sy}");
    }
}
