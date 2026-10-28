//! Merging photos on the CPU: an exposure bracket into one photo ([`fuse`], exposure fusion
//! after Mertens, Kautz and Van Reeth, the method Enfuse uses), after lining the frames up
//! ([`align`], Ward's median threshold bitmaps, which do not care how bright each frame is).
//!
//! Exposure fusion blends the frames directly: each pixel is taken mostly from the frames
//! where it is well exposed, saturated and detailed, blended through Laplacian pyramids so the
//! seams do not show. It needs no exposure times and gives a finished, natural-looking photo
//! (no tone mapping step), which the studio then edits like any other.
//!
//! Images are [`Rgb`]: linear rows of interleaved `f32` red, green, blue, display-referred
//! (0..1, as the develop pass outputs them).

use rayon::prelude::*;

/// An RGB image, `w`×`h`, interleaved `f32` values in 0..1.
#[derive(Clone, Debug, PartialEq)]
pub struct Rgb {
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

impl Rgb {
    pub fn new(w: usize, h: usize) -> Rgb {
        Rgb {
            w,
            h,
            data: vec![0.0; w * h * 3],
        }
    }

    fn px(&self, x: usize, y: usize) -> [f32; 3] {
        let i = (y * self.w + x) * 3;
        [self.data[i], self.data[i + 1], self.data[i + 2]]
    }

    /// The image moved by (`dx`, `dy`) pixels; uncovered edges repeat the nearest pixel.
    pub fn shifted(&self, dx: i32, dy: i32) -> Rgb {
        if dx == 0 && dy == 0 {
            return self.clone();
        }
        let mut out = Rgb::new(self.w, self.h);
        out.data
            .par_chunks_mut(self.w * 3)
            .enumerate()
            .for_each(|(y, row)| {
                let sy = (y as i32 - dy).clamp(0, self.h as i32 - 1) as usize;
                for x in 0..self.w {
                    let sx = (x as i32 - dx).clamp(0, self.w as i32 - 1) as usize;
                    row[x * 3..x * 3 + 3].copy_from_slice(&self.px(sx, sy));
                }
            });
        out
    }
}

/// A one-channel image.
#[derive(Clone, Debug)]
struct Plane {
    w: usize,
    h: usize,
    data: Vec<f32>,
}

fn luma(p: [f32; 3]) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

fn grey(img: &Rgb) -> Plane {
    Plane {
        w: img.w,
        h: img.h,
        data: img
            .data
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| luma(*p))
            .collect(),
    }
}

/// Half the size, each pixel the mean of the 2×2 below it.
fn halve(p: &Plane) -> Plane {
    let (w, h) = ((p.w / 2).max(1), (p.h / 2).max(1));
    let mut data = vec![0.0; w * h];
    for y in 0..h {
        for x in 0..w {
            let (x0, y0) = ((x * 2).min(p.w - 1), (y * 2).min(p.h - 1));
            let (x1, y1) = ((x0 + 1).min(p.w - 1), (y0 + 1).min(p.h - 1));
            data[y * w + x] = (p.data[y0 * p.w + x0]
                + p.data[y0 * p.w + x1]
                + p.data[y1 * p.w + x0]
                + p.data[y1 * p.w + x1])
                / 4.0;
        }
    }
    Plane { w, h, data }
}

/// Ward's median threshold bitmap and its exclusion mask (pixels too close to the median to
/// trust) for one pyramid level.
fn bitmaps(p: &Plane) -> (Vec<bool>, Vec<bool>) {
    let mut sorted = p.data.clone();
    let mid = sorted.len() / 2;
    let (_, median, _) = sorted.select_nth_unstable_by(mid, f32::total_cmp);
    let median = *median;
    let threshold = p.data.iter().map(|&v| v > median).collect();
    let keep = p
        .data
        .iter()
        .map(|&v| (v - median).abs() > 4.0 / 255.0)
        .collect();
    (threshold, keep)
}

/// How many pixels differ between `a` and `b` moved by (`dx`, `dy`), where both are trusted.
fn differences(
    a: &(Vec<bool>, Vec<bool>),
    b: &(Vec<bool>, Vec<bool>),
    w: usize,
    h: usize,
    dx: i32,
    dy: i32,
) -> usize {
    let mut n = 0;
    for y in 0..h as i32 {
        let sy = y - dy;
        if sy < 0 || sy >= h as i32 {
            continue;
        }
        for x in 0..w as i32 {
            let sx = x - dx;
            if sx < 0 || sx >= w as i32 {
                continue;
            }
            let i = (y as usize) * w + x as usize;
            let j = (sy as usize) * w + sx as usize;
            if a.1[i] && b.1[j] && a.0[i] != b.0[j] {
                n += 1;
            }
        }
    }
    n
}

/// The shift (`dx`, `dy`) that lines `frame` up with `reference`, found coarse to fine over a
/// pyramid (`levels` halvings: up to ±2^`levels` pixels). Brightness does not matter: each
/// level compares median threshold bitmaps.
pub fn align(reference: &Rgb, frame: &Rgb, levels: u32) -> (i32, i32) {
    let mut pa = vec![grey(reference)];
    let mut pb = vec![grey(frame)];
    for _ in 0..levels {
        let (a, b) = (halve(pa.last().unwrap()), halve(pb.last().unwrap()));
        if a.w < 16 || a.h < 16 {
            break;
        }
        pa.push(a);
        pb.push(b);
    }
    let (mut dx, mut dy) = (0i32, 0i32);
    for (a, b) in pa.iter().zip(&pb).rev() {
        dx *= 2;
        dy *= 2;
        let (ba, bb) = (bitmaps(a), bitmaps(b));
        let mut best = (usize::MAX, dx, dy);
        for oy in -1..=1 {
            for ox in -1..=1 {
                let n = differences(&ba, &bb, a.w, a.h, dx + ox, dy + oy);
                if n < best.0 {
                    best = (n, dx + ox, dy + oy);
                }
            }
        }
        (dx, dy) = (best.1, best.2);
    }
    (dx, dy)
}

/// The fusion's weight for each pixel: contrast (the grey image's Laplacian), saturation
/// (the channels' spread) and well-exposedness (closeness to mid grey), multiplied.
fn weights(img: &Rgb) -> Vec<f32> {
    let g = grey(img);
    let (w, h) = (img.w, img.h);
    let mut out = vec![0.0; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, cell) in row.iter_mut().enumerate() {
            let at = |dx: i32, dy: i32| {
                let xx = (x as i32 + dx).clamp(0, w as i32 - 1) as usize;
                let yy = (y as i32 + dy).clamp(0, h as i32 - 1) as usize;
                g.data[yy * w + xx]
            };
            let contrast = (at(-1, 0) + at(1, 0) + at(0, -1) + at(0, 1) - 4.0 * at(0, 0)).abs();
            let p = img.px(x, y);
            let mean = (p[0] + p[1] + p[2]) / 3.0;
            let saturation =
                (((p[0] - mean).powi(2) + (p[1] - mean).powi(2) + (p[2] - mean).powi(2)) / 3.0)
                    .sqrt();
            let exposed: f32 = p
                .iter()
                .map(|v| (-(v - 0.5).powi(2) / (2.0 * 0.2 * 0.2)).exp())
                .product();
            // Small floors, so a flat grey frame still counts and no pixel has no weight.
            *cell = (contrast + 0.002) * (saturation + 0.002) * exposed + 1e-12;
        }
    });
    out
}

/// A pyramid level is downsampled with a 5-tap binomial blur, and upsampled bilinearly; the
/// Laplacian uses the same upsampling as the collapse, so the round trip is exact.
fn blur_down<const C: usize>(src: &[f32], w: usize, h: usize) -> (Vec<f32>, usize, usize) {
    const K: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];
    // Horizontal pass, keeping every other column.
    let nw = w.div_ceil(2);
    let mut tmp = vec![0.0; nw * h * C];
    tmp.par_chunks_mut(nw * C).enumerate().for_each(|(y, row)| {
        for nx in 0..nw {
            let x = nx * 2;
            for (k, kv) in K.iter().enumerate() {
                let sx = (x as i32 + k as i32 - 2).clamp(0, w as i32 - 1) as usize;
                for c in 0..C {
                    row[nx * C + c] += kv * src[(y * w + sx) * C + c];
                }
            }
        }
    });
    // Vertical pass, keeping every other row.
    let nh = h.div_ceil(2);
    let mut out = vec![0.0; nw * nh * C];
    out.par_chunks_mut(nw * C)
        .enumerate()
        .for_each(|(ny, row)| {
            let y = ny * 2;
            for (k, kv) in K.iter().enumerate() {
                let sy = (y as i32 + k as i32 - 2).clamp(0, h as i32 - 1) as usize;
                let src_row = &tmp[sy * nw * C..(sy + 1) * nw * C];
                for (o, s) in row.iter_mut().zip(src_row) {
                    *o += kv * s;
                }
            }
        });
    (out, nw, nh)
}

fn up<const C: usize>(src: &[f32], sw: usize, sh: usize, w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0.0; w * h * C];
    out.par_chunks_mut(w * C).enumerate().for_each(|(y, row)| {
        let fy = ((y as f32 + 0.5) / 2.0 - 0.5).clamp(0.0, (sh - 1) as f32);
        let (y0, ty) = (fy.floor() as usize, fy - fy.floor());
        let y1 = (y0 + 1).min(sh - 1);
        for x in 0..w {
            let fx = ((x as f32 + 0.5) / 2.0 - 0.5).clamp(0.0, (sw - 1) as f32);
            let (x0, tx) = (fx.floor() as usize, fx - fx.floor());
            let x1 = (x0 + 1).min(sw - 1);
            for c in 0..C {
                let s = |xx: usize, yy: usize| src[(yy * sw + xx) * C + c];
                row[x * C + c] = (s(x0, y0) * (1.0 - tx) + s(x1, y0) * tx) * (1.0 - ty)
                    + (s(x0, y1) * (1.0 - tx) + s(x1, y1) * tx) * ty;
            }
        }
    });
    out
}

/// The pyramid's levels for a `w`×`h` image: down to about 8 pixels on the short side.
fn level_count(w: usize, h: usize) -> usize {
    let short = w.min(h).max(1) as f32;
    ((short / 8.0).log2().floor() as usize).clamp(1, 12)
}

/// Laplacian pyramid of a `C`-channel image (each level with its size); the last level is
/// the smallest Gaussian level.
fn laplacian<const C: usize>(
    img: Vec<f32>,
    w: usize,
    h: usize,
    levels: usize,
) -> Vec<(Vec<f32>, usize, usize)> {
    let mut out = Vec::with_capacity(levels);
    let (mut cur, mut cw, mut ch) = (img, w, h);
    for _ in 0..levels - 1 {
        let (small, sw, sh) = blur_down::<C>(&cur, cw, ch);
        let expanded = up::<C>(&small, sw, sh, cw, ch);
        let lap: Vec<f32> = cur.iter().zip(&expanded).map(|(a, b)| a - b).collect();
        out.push((lap, cw, ch));
        (cur, cw, ch) = (small, sw, sh);
    }
    out.push((cur, cw, ch));
    out
}

fn gaussian(img: Vec<f32>, w: usize, h: usize, levels: usize) -> Vec<Vec<f32>> {
    let mut out = Vec::with_capacity(levels);
    let (mut cur, mut cw, mut ch) = (img, w, h);
    for _ in 0..levels - 1 {
        let (small, sw, sh) = blur_down::<1>(&cur, cw, ch);
        out.push(cur);
        (cur, cw, ch) = (small, sw, sh);
    }
    out.push(cur);
    out
}

/// Why a merge cannot be made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeError {
    TooFew,
    SizesDiffer,
}

impl std::fmt::Display for MergeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            MergeError::TooFew => "A merge needs at least two photos.",
            MergeError::SizesDiffer => "The photos are not all the same size.",
        })
    }
}

impl std::error::Error for MergeError {}

/// Fuses the frames (already lined up) into one, by exposure fusion. `frame(i)` gives frame
/// `i` of `count`; it is asked for twice (weights, then blending), so a caller short of
/// memory can render each again instead of keeping them all.
pub fn fuse(count: usize, mut frame: impl FnMut(usize) -> Rgb) -> Result<Rgb, MergeError> {
    if count < 2 {
        return Err(MergeError::TooFew);
    }
    // First pass: each frame's weights, and their sum.
    let mut all = Vec::with_capacity(count);
    let (mut w, mut h) = (0, 0);
    for i in 0..count {
        let img = frame(i);
        if i == 0 {
            (w, h) = (img.w, img.h);
        } else if (img.w, img.h) != (w, h) {
            return Err(MergeError::SizesDiffer);
        }
        all.push(weights(&img));
    }
    let mut total = vec![0.0f32; w * h];
    for wt in &all {
        for (t, v) in total.iter_mut().zip(wt) {
            *t += v;
        }
    }
    let levels = level_count(w, h);
    // Second pass: each frame's Laplacian pyramid, weighted by its normalised weights'
    // Gaussian pyramid, added up.
    let mut acc: Option<Vec<(Vec<f32>, usize, usize)>> = None;
    for (i, wt) in all.into_iter().enumerate() {
        let normalised: Vec<f32> = wt.iter().zip(&total).map(|(a, t)| a / t).collect();
        let gw = gaussian(normalised, w, h, levels);
        let img = frame(i);
        let lap = laplacian::<3>(img.data, w, h, levels);
        let weighted: Vec<(Vec<f32>, usize, usize)> = lap
            .into_iter()
            .zip(&gw)
            .map(|((l, lw, lh), g)| {
                let v: Vec<f32> = l
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .zip(g)
                    .flat_map(|(p, k)| [p[0] * k, p[1] * k, p[2] * k])
                    .collect();
                (v, lw, lh)
            })
            .collect();
        acc = Some(match acc {
            None => weighted,
            Some(mut sum) => {
                for ((s, _, _), (v, _, _)) in sum.iter_mut().zip(&weighted) {
                    for (a, b) in s.iter_mut().zip(v) {
                        *a += b;
                    }
                }
                sum
            }
        });
    }
    // Collapse, smallest level first.
    let mut levels_rev = acc.expect("two or more frames").into_iter().rev();
    let (mut cur, mut cw, mut ch) = levels_rev.next().expect("one level at least");
    for (lap, lw, lh) in levels_rev {
        let expanded = up::<3>(&cur, cw, ch, lw, lh);
        cur = lap.iter().zip(&expanded).map(|(a, b)| a + b).collect();
        (cw, ch) = (lw, lh);
    }
    for v in &mut cur {
        *v = v.clamp(0.0, 1.0);
    }
    Ok(Rgb {
        w: cw,
        h: ch,
        data: cur,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A repeatable value in 0..1 for a block (no pattern a shift could match).
    fn noise(x: i32, y: i32) -> f32 {
        let mut n = (x as u32).wrapping_mul(374_761_393) ^ (y as u32).wrapping_mul(668_265_263);
        n = (n ^ (n >> 13)).wrapping_mul(1_274_126_177);
        (n >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Blocks of every brightness over the whole frame, times `gain`, moved by `shift`.
    fn textured(w: usize, h: usize, gain: f32, shift: (i32, i32)) -> Rgb {
        let mut img = Rgb::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let (sx, sy) = (x as i32 - shift.0, y as i32 - shift.1);
                let v = (0.05 + 0.9 * noise(sx.div_euclid(9), sy.div_euclid(9))) * gain;
                let i = (y * w + x) * 3;
                img.data[i..i + 3].copy_from_slice(&[v, v, v]);
            }
        }
        img
    }

    /// A scene with detail in the shadows and the highlights, seen at `stops` of exposure.
    fn scene(w: usize, h: usize, stops: f32, shift: (i32, i32)) -> Rgb {
        let mut img = Rgb::new(w, h);
        let gain = 2f32.powf(stops);
        for y in 0..h {
            for x in 0..w {
                let (sx, sy) = (x as i32 - shift.0, y as i32 - shift.1);
                // Left half deep shadow, right half bright sky; blocks of texture in both.
                let base = if sx < w as i32 / 2 { 0.02 } else { 0.6 };
                let detail = 0.7 + 0.6 * noise(sx.div_euclid(5), sy.div_euclid(5));
                let v = (base * detail * gain).clamp(0.0, 1.0);
                let i = (y * w + x) * 3;
                img.data[i..i + 3].copy_from_slice(&[v, v * 0.9, v * 0.8]);
            }
        }
        img
    }

    #[test]
    fn the_shift_between_frames_is_found_whatever_their_brightness() {
        let reference = textured(256, 192, 1.0, (0, 0));
        let darker = textured(256, 192, 0.25, (5, -3));
        let (dx, dy) = align(&reference, &darker, 5);
        assert_eq!((dx, dy), (-5, 3), "moving the frame by this lines it up");
        let lined = darker.shifted(dx, dy);
        let want = textured(256, 192, 0.25, (0, 0)).px(100, 100)[0];
        assert!((lined.px(100, 100)[0] - want).abs() < 1e-6);
    }

    #[test]
    fn a_pyramid_round_trip_is_exact() {
        let img = scene(97, 61, 0.0, (0, 0));
        let levels = level_count(97, 61);
        let lap = laplacian::<3>(img.data.clone(), 97, 61, levels);
        let mut rev = lap.into_iter().rev();
        let (mut cur, mut cw, mut ch) = rev.next().unwrap();
        for (l, lw, lh) in rev {
            let e = up::<3>(&cur, cw, ch, lw, lh);
            cur = l.iter().zip(&e).map(|(a, b)| a + b).collect();
            (cw, ch) = (lw, lh);
        }
        let worst = cur
            .iter()
            .zip(&img.data)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-5, "{worst}");
    }

    #[test]
    fn fusion_keeps_the_shadows_from_the_bright_frame_and_the_sky_from_the_dark_one() {
        let frames = [
            scene(128, 96, -2.0, (0, 0)),
            scene(128, 96, 0.0, (0, 0)),
            scene(128, 96, 3.0, (0, 0)),
        ];
        let out = fuse(3, |i| frames[i].clone()).unwrap();
        assert_eq!((out.w, out.h), (128, 96));
        let shadow = out.px(20, 50)[0];
        let sky = out.px(110, 50)[0];
        // The middle frame's shadows are near black and its sky near clipping; the fused
        // photo lifts one and holds the other.
        assert!(shadow > frames[1].px(20, 50)[0] * 3.0, "{shadow}");
        assert!(sky < 0.95, "{sky}");
        // Detail survives in both halves.
        let spread = |x0: usize| {
            let v: Vec<f32> = (x0..x0 + 12).map(|x| out.px(x, 40)[0]).collect();
            v.iter().cloned().fold(0.0, f32::max) - v.iter().cloned().fold(1.0, f32::min)
        };
        assert!(spread(20) > 0.02 && spread(100) > 0.02);
    }

    #[test]
    fn a_merge_needs_two_frames_of_one_size() {
        assert_eq!(fuse(1, |_| Rgb::new(4, 4)).unwrap_err(), MergeError::TooFew);
        let sizes = [Rgb::new(16, 16), Rgb::new(16, 8)];
        assert_eq!(
            fuse(2, |i| sizes[i].clone()).unwrap_err(),
            MergeError::SizesDiffer
        );
    }
}
