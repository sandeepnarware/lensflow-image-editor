//! Panoramas: a row of photos, taken turning the camera left to right (or right to left),
//! stitched into one wide photo.
//!
//! Each frame is projected onto a cylinder (so turning the camera becomes sliding the
//! picture), each neighbour's offset is found by normalised cross-correlation (brightness
//! differences do not matter), coarse to fine over a small pyramid, and the frames are laid
//! on one canvas, evened out in brightness, blended with feathered edges and cropped to the
//! rectangle every column covers. The cylinder's radius (the lens's focal length in pixels)
//! is not known from the files, so it is found too: each candidate is tried on small copies
//! and the one whose overlaps match best wins (a flat scene, or a long lens, picks "no
//! cylinder").
//!
//! One row only: a panorama of several rows, or one shot turning up and down, is not
//! stitched. Moving subjects in an overlap may show twice, faintly.

use rayon::prelude::*;

use crate::{MergeError, Rgb};

/// The registration's sizes: the search runs at the first height, then each offset is
/// refined at the others.
const HEIGHTS: [usize; 3] = [96, 192, 384];
/// The largest panorama written, in pixels (bigger ones are made smaller).
pub const MAX_PIXELS: usize = 60_000_000;

/// A neighbour whose best match is worse than this (1 − correlation) is not a match.
const MAX_MISMATCH: f32 = 0.5;

/// The cylinder radii tried, as multiples of a frame's width (`None`: no cylinder).
const FOCALS: [Option<f32>; 13] = [
    Some(0.6),
    Some(0.7),
    Some(0.8),
    Some(0.9),
    Some(1.0),
    Some(1.15),
    Some(1.3),
    Some(1.5),
    Some(1.8),
    Some(2.2),
    Some(2.8),
    Some(3.6),
    None,
];

/// A grey image with the pixels a projection left empty marked.
#[derive(Clone)]
struct Grey {
    w: usize,
    h: usize,
    v: Vec<f32>,
    ok: Vec<bool>,
}

/// `img`'s grey, `height` pixels tall (box filtered), all valid.
fn small(img: &Rgb, height: usize) -> Grey {
    let height = height.min(img.h).max(8);
    let k = img.h as f32 / height as f32;
    let w = ((img.w as f32 / k).round() as usize).max(8);
    let mut v = vec![0.0; w * height];
    v.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let (y0, y1) = (
            (y as f32 * k) as usize,
            (((y + 1) as f32 * k) as usize).min(img.h),
        );
        for (x, cell) in row.iter_mut().enumerate() {
            let (x0, x1) = (
                (x as f32 * k) as usize,
                (((x + 1) as f32 * k) as usize).min(img.w),
            );
            let (mut sum, mut n) = (0.0, 0.0);
            for yy in y0..y1.max(y0 + 1).min(img.h) {
                for xx in x0..x1.max(x0 + 1).min(img.w) {
                    let i = (yy * img.w + xx) * 3;
                    sum += crate::luma([img.data[i], img.data[i + 1], img.data[i + 2]]);
                    n += 1.0;
                }
            }
            *cell = sum / n;
        }
    });
    Grey {
        w,
        h: height,
        v,
        ok: vec![true; w * height],
    }
}

/// Where the cylinder pixel (`xc`, `yc`) comes from in a `w`×`h` frame of radius `f`
/// (pixels), or `None` outside it.
fn from_cylinder(xc: f32, yc: f32, w: usize, h: usize, f: Option<f32>) -> Option<(f32, f32)> {
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let (sx, sy) = match f {
        None => (xc, yc),
        Some(f) => {
            let theta = (xc - cx) / f;
            if theta.abs() >= std::f32::consts::FRAC_PI_2 * 0.99 {
                return None;
            }
            (cx + f * theta.tan(), cy + (yc - cy) / theta.cos())
        }
    };
    (sx >= 0.0 && sy >= 0.0 && sx <= (w - 1) as f32 && sy <= (h - 1) as f32).then_some((sx, sy))
}

fn bilinear(v: &[f32], w: usize, x: f32, y: f32, c: usize, channels: usize) -> f32 {
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (tx, ty) = (x - x0 as f32, y - y0 as f32);
    let h = v.len() / channels / w;
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let at = |xx: usize, yy: usize| v[(yy * w + xx) * channels + c];
    (at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx) * (1.0 - ty)
        + (at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx) * ty
}

/// `g` projected onto a cylinder of radius `f` (as a multiple of its width).
fn warp_grey(g: &Grey, f: Option<f32>) -> Grey {
    let fp = f.map(|m| m * g.w as f32);
    let mut out = g.clone();
    for y in 0..g.h {
        for x in 0..g.w {
            let i = y * g.w + x;
            match from_cylinder(x as f32, y as f32, g.w, g.h, fp) {
                Some((sx, sy)) => {
                    out.v[i] = bilinear(&g.v, g.w, sx, sy, 0, 1);
                    out.ok[i] = true;
                }
                None => {
                    out.v[i] = 0.0;
                    out.ok[i] = false;
                }
            }
        }
    }
    out
}

/// How badly `b` placed at (`dx`, `dy`) on `a` matches (1 − normalised cross-correlation over
/// the overlap), and the overlap's mean brightness in each; `None` when they overlap by less
/// than a tenth of a frame.
fn mismatch(a: &Grey, b: &Grey, dx: i32, dy: i32) -> Option<(f32, f32, f32)> {
    let (mut n, mut sa, mut sb, mut saa, mut sbb, mut sab) = (0f32, 0f32, 0f32, 0f32, 0f32, 0f32);
    let x0 = dx.max(0);
    let x1 = (a.w as i32).min(dx + b.w as i32);
    let y0 = dy.max(0);
    let y1 = (a.h as i32).min(dy + b.h as i32);
    // Every other pixel is plenty for a match.
    for y in (y0..y1).step_by(2) {
        for x in (x0..x1).step_by(2) {
            let i = y as usize * a.w + x as usize;
            let j = (y - dy) as usize * b.w + (x - dx) as usize;
            if !(a.ok[i] && b.ok[j]) {
                continue;
            }
            let (p, q) = (a.v[i], b.v[j]);
            n += 1.0;
            sa += p;
            sb += q;
            saa += p * p;
            sbb += q * q;
            sab += p * q;
        }
    }
    if n < (a.w * a.h) as f32 / 40.0 {
        return None;
    }
    let (ma, mb) = (sa / n, sb / n);
    let va = (saa / n - ma * ma).max(1e-8);
    let vb = (sbb / n - mb * mb).max(1e-8);
    let ncc = (sab / n - ma * mb) / (va * vb).sqrt();
    Some((1.0 - ncc, ma, mb))
}

/// The best offset of `b` on `a` over every overlap from a twentieth to nineteen twentieths
/// of a frame, either side, and up to a tenth up or down.
fn search(a: &Grey, b: &Grey) -> Option<(i32, i32, f32)> {
    let (w, h) = (a.w as i32, a.h as i32);
    let dys: Vec<i32> = (-(h / 10)..=h / 10).collect();
    let dxs: Vec<i32> = (w / 20..=w * 19 / 20).flat_map(|d| [d, -d]).collect();
    dxs.par_iter()
        .flat_map_iter(|&dx| dys.iter().map(move |&dy| (dx, dy)))
        .filter_map(|(dx, dy)| mismatch(a, b, dx, dy).map(|(e, _, _)| (dx, dy, e)))
        .min_by(|p, q| p.2.total_cmp(&q.2))
        .filter(|best| best.2 <= MAX_MISMATCH)
}

/// The offset near (`dx`, `dy`) (±2) that matches best.
fn refine(a: &Grey, b: &Grey, dx: i32, dy: i32) -> (i32, i32) {
    let mut best = (f32::MAX, dx, dy);
    for oy in -2..=2 {
        for ox in -2..=2 {
            if let Some((e, _, _)) = mismatch(a, b, dx + ox, dy + oy)
                && e < best.0
            {
                best = (e, dx + ox, dy + oy);
            }
        }
    }
    (best.1, best.2)
}

/// Lines up and blends `frames` (neighbours in order, all one size) into one panorama.
/// `progress(done, total)` as it goes.
pub fn stitch(frames: &[Rgb], mut progress: impl FnMut(usize, usize)) -> Result<Rgb, MergeError> {
    if frames.len() < 2 {
        return Err(MergeError::TooFew);
    }
    let (fw, fh) = (frames[0].w, frames[0].h);
    if frames.iter().any(|f| (f.w, f.h) != (fw, fh)) {
        return Err(MergeError::SizesDiffer);
    }
    let total = 3 + frames.len();
    progress(0, total);
    // The small copies at each registration height.
    let smalls: Vec<Vec<Grey>> = HEIGHTS
        .iter()
        .map(|&h| frames.iter().map(|f| small(f, h)).collect())
        .collect();
    // Each cylinder: the neighbours' offsets searched on the smallest copies, refined on the
    // larger ones, and scored on the largest (the smallest is too coarse to judge by).
    struct Fit {
        focal: Option<f32>,
        score: f32,
        offsets: Vec<(i32, i32)>,
        /// Each neighbour's overlap brightness: (left frame's, right frame's).
        means: Vec<(f32, f32)>,
    }
    let fit = |focal: Option<f32>| -> Option<Fit> {
        let warped: Vec<Grey> = smalls[0].iter().map(|g| warp_grey(g, focal)).collect();
        let mut offsets = Vec::new();
        for pair in warped.windows(2) {
            let (dx, dy, _) = search(&pair[0], &pair[1])?;
            offsets.push((dx, dy));
        }
        let mut score = 0.0;
        let mut means = Vec::new();
        for level in 1..HEIGHTS.len() {
            // The sizes as made (a frame smaller than a height is not enlarged).
            let k = smalls[level][0].h as f32 / smalls[level - 1][0].h as f32;
            let warped: Vec<Grey> = smalls[level].iter().map(|g| warp_grey(g, focal)).collect();
            for (i, o) in offsets.iter_mut().enumerate() {
                let guess = (
                    (o.0 as f32 * k).round() as i32,
                    (o.1 as f32 * k).round() as i32,
                );
                *o = refine(&warped[i], &warped[i + 1], guess.0, guess.1);
                if level == HEIGHTS.len() - 1 {
                    let (e, ma, mb) = mismatch(&warped[i], &warped[i + 1], o.0, o.1)?;
                    if e > MAX_MISMATCH {
                        return None;
                    }
                    score += e;
                    means.push((ma, mb));
                }
            }
        }
        Some(Fit {
            focal,
            score,
            offsets,
            means,
        })
    };
    let mut fits: Vec<Fit> = FOCALS.iter().filter_map(|&f| fit(f)).collect();
    let best = fits
        .iter()
        .map(|f| f.score)
        .min_by(f32::total_cmp)
        .ok_or(MergeError::NoOverlap)?;
    // No cylinder unless one matches clearly better: a flat scene or a long lens should not
    // be bent by a near tie.
    let chosen = match fits.iter().position(|f| f.focal.is_none()) {
        Some(flat) if fits[flat].score <= best * 1.1 + 1e-3 => fits.swap_remove(flat),
        _ => fits
            .into_iter()
            .min_by(|a, b| a.score.total_cmp(&b.score))
            .expect("one fit"),
    };
    progress(1, total);
    let (focal, offsets) = (chosen.focal, chosen.offsets);
    // Even the brightness: each frame matched to its left neighbour in their overlap, then
    // all around their geometric mean.
    let mut gains = vec![1.0f32; frames.len()];
    for (i, (ma, mb)) in chosen.means.iter().enumerate() {
        gains[i + 1] = gains[i] * (ma / mb.max(1e-4));
    }
    let mean = (gains.iter().map(|g| g.ln()).sum::<f32>() / gains.len() as f32).exp();
    for g in &mut gains {
        *g /= mean;
    }
    log::debug!("panorama: cylinder {focal:?}, offsets {offsets:?}, gains {gains:?}");
    progress(2, total);

    // Placement at full size, and the canvas.
    let k = fh as f32 / smalls[HEIGHTS.len() - 1][0].h as f32;
    let mut places = vec![(0f32, 0f32)];
    for o in &offsets {
        let last = *places.last().expect("one place");
        places.push((last.0 + o.0 as f32 * k, last.1 + o.1 as f32 * k));
    }
    let min_x = places.iter().map(|p| p.0).fold(f32::MAX, f32::min);
    let min_y = places.iter().map(|p| p.1).fold(f32::MAX, f32::min);
    let max_x = places.iter().map(|p| p.0).fold(f32::MIN, f32::max) + fw as f32;
    let max_y = places.iter().map(|p| p.1).fold(f32::MIN, f32::max) + fh as f32;
    let area = (max_x - min_x) * (max_y - min_y);
    let scale = (MAX_PIXELS as f32 / area).sqrt().min(1.0);
    let cw = ((max_x - min_x) * scale).ceil() as usize;
    let ch = ((max_y - min_y) * scale).ceil() as usize;
    let fp = focal.map(|m| m * fw as f32);
    let mut acc = vec![0f32; cw * ch * 3];
    let mut weight = vec![0f32; cw * ch];
    for (i, frame) in frames.iter().enumerate() {
        let (px, py) = ((places[i].0 - min_x) * scale, (places[i].1 - min_y) * scale);
        // The frame's columns on the canvas, and its valid width on the cylinder.
        let (half, cx) = (
            match fp {
                Some(f) => f * ((fw as f32 / 2.0) / f).atan(),
                None => fw as f32 / 2.0,
            },
            fw as f32 / 2.0,
        );
        let x_start = px.floor().max(0.0) as usize;
        let x_end = ((px + fw as f32 * scale).ceil() as usize).min(cw);
        let y_start = py.floor().max(0.0) as usize;
        let y_end = ((py + fh as f32 * scale).ceil() as usize).min(ch);
        let gain = gains[i];
        // Row by row in parallel: each canvas row is written by one task.
        let rows: Vec<(usize, Vec<f32>, Vec<f32>)> = (y_start..y_end)
            .into_par_iter()
            .map(|y| {
                let mut rgb = vec![0f32; (x_end - x_start) * 3];
                let mut wt = vec![0f32; x_end - x_start];
                for x in x_start..x_end {
                    let (xc, yc) = ((x as f32 - px) / scale, (y as f32 - py) / scale);
                    let Some((sx, sy)) = from_cylinder(xc, yc, fw, fh, fp) else {
                        continue;
                    };
                    // Feathered: strongest in the middle of the frame, none at its edges.
                    let edge = (half - (xc - cx).abs()).max(0.0);
                    if edge <= 0.0 {
                        continue;
                    }
                    let j = x - x_start;
                    for c in 0..3 {
                        rgb[j * 3 + c] = bilinear(&frame.data, fw, sx, sy, c, 3) * gain * edge;
                    }
                    wt[j] = edge;
                }
                (y, rgb, wt)
            })
            .collect();
        for (y, rgb, wt) in rows {
            let base = y * cw + x_start;
            for (j, w) in wt.iter().enumerate() {
                if *w > 0.0 {
                    weight[base + j] += w;
                    for c in 0..3 {
                        acc[(base + j) * 3 + c] += rgb[j * 3 + c];
                    }
                }
            }
        }
        progress(3 + i, total);
    }
    // Crop to the rows every covered column has, and the columns any frame covers.
    let mut top = 0usize;
    let mut bottom = ch;
    let (mut left, mut right) = (cw, 0usize);
    for x in 0..cw {
        let first = (0..ch).find(|&y| weight[y * cw + x] > 0.0);
        let Some(first) = first else { continue };
        let last = (0..ch)
            .rev()
            .find(|&y| weight[y * cw + x] > 0.0)
            .unwrap_or(first);
        left = left.min(x);
        right = right.max(x + 1);
        top = top.max(first);
        bottom = bottom.min(last + 1);
    }
    if bottom <= top || right <= left {
        return Err(MergeError::NoOverlap);
    }
    let (ow, oh) = (right - left, bottom - top);
    let mut out = Rgb::new(ow, oh);
    out.data
        .par_chunks_mut(ow * 3)
        .enumerate()
        .for_each(|(y, row)| {
            for x in 0..ow {
                let i = (y + top) * cw + x + left;
                let w = weight[i];
                for c in 0..3 {
                    row[x * 3 + c] = if w > 0.0 {
                        (acc[i * 3 + c] / w).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                }
            }
        });
    progress(total, total);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(x: i32, y: i32) -> f32 {
        let mut n = (x as u32).wrapping_mul(374_761_393) ^ (y as u32).wrapping_mul(668_265_263);
        n = (n ^ (n >> 13)).wrapping_mul(1_274_126_177);
        (n >> 8) as f32 / (1u32 << 24) as f32
    }

    /// A wide flat scene of textured blocks.
    fn scene(w: usize, h: usize) -> Rgb {
        let mut img = Rgb::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = 0.1 + 0.8 * noise(x as i32 / 12, y as i32 / 12);
                let i = (y * w + x) * 3;
                img.data[i..i + 3].copy_from_slice(&[v, v * 0.8 + 0.1, 1.0 - v]);
            }
        }
        img
    }

    fn crop(img: &Rgb, x0: usize, y0: usize, w: usize, h: usize, gain: f32) -> Rgb {
        let mut out = Rgb::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let p = img.px(x0 + x, y0 + y);
                let i = (y * w + x) * 3;
                out.data[i..i + 3].copy_from_slice(&p.map(|v| (v * gain).min(1.0)));
            }
        }
        out
    }

    #[test]
    fn three_overlapping_frames_of_a_flat_scene_stitch_back_into_it() {
        let world = scene(900, 300);
        // 400 wide frames every 250 px (150 px of overlap), the middle a little brighter
        // and a few pixels lower.
        let frames = vec![
            crop(&world, 0, 10, 400, 260, 1.0),
            crop(&world, 250, 14, 400, 260, 1.1),
            crop(&world, 500, 10, 400, 260, 1.0),
        ];
        let mut steps = Vec::new();
        let pano = stitch(&frames, |d, t| steps.push((d, t))).unwrap();
        assert!((pano.w as i32 - 900).abs() <= 6, "{}", pano.w);
        assert!(pano.h >= 250 && pano.h <= 260, "{}", pano.h);
        assert_eq!(steps.last(), Some(&(6, 6)));
        // Away from the borders, the panorama is the scene (the middle frame's extra
        // brightness evened out, the feathering and resampling aside).
        // The panorama's top row is the scene's row 14 (the crop of the lower frame); a pixel
        // or two of placement is allowed.
        for (x, y) in [(120, 100), (330, 120), (450, 140), (620, 90), (800, 160)] {
            let got = pano.px(x, y)[0];
            let near = (-2..=2)
                .flat_map(|dy| (-2..=2).map(move |dx| (dx, dy)))
                .map(|(dx, dy)| {
                    let (wx, wy) = ((x as i32 + dx) as usize, (y as i32 + 14 + dy) as usize);
                    (world.px(wx, wy)[0] - got).abs()
                })
                .fold(f32::MAX, f32::min);
            assert!(near < 0.06, "({x}, {y}): {near}");
        }
    }

    #[test]
    fn frames_turned_with_a_wide_lens_stitch_on_their_cylinder() {
        // The scene on a cylinder of radius f = the frame's width (a wide lens), three frames
        // taken turning 250 cylinder pixels each.
        let (w, h, f) = (400usize, 300usize, 400f32);
        let world = scene(1000, 400);
        let frame = |turn: f32| {
            let mut img = Rgb::new(w, h);
            let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
            for y in 0..h {
                for x in 0..w {
                    let theta = ((x as f32 - cx) / f).atan();
                    let u = 60.0 + turn + cx + f * theta;
                    let v = 50.0 + cy + (y as f32 - cy) * theta.cos();
                    let i = (y * w + x) * 3;
                    for c in 0..3 {
                        img.data[i + c] = bilinear(&world.data, world.w, u, v, c, 3);
                    }
                }
            }
            img
        };
        let frames = vec![frame(0.0), frame(250.0), frame(500.0)];
        let pano = stitch(&frames, |_, _| {}).unwrap();
        // One offset must fit points across both seams: the stitch is the cylinder.
        let points: Vec<(usize, usize)> = (0..40)
            .map(|i| (20 + i * (pano.w - 40) / 40, 30 + (i * 37) % (pano.h - 60)))
            .collect();
        let mut best = f32::MAX;
        for oy in 0..120 {
            for ox in 0..140 {
                let err: f32 = points
                    .iter()
                    .map(|&(x, y)| (pano.px(x, y)[0] - world.px(x + ox, y + oy)[0]).abs())
                    .sum::<f32>()
                    / points.len() as f32;
                best = best.min(err);
            }
        }
        assert!(best < 0.05, "{best}");
    }

    #[test]
    fn frames_that_do_not_overlap_are_refused() {
        let a = scene(200, 100);
        let mut b = Rgb::new(200, 100);
        b.data.fill(0.5);
        assert!(stitch(&[a, b], |_, _| {}).is_err());
    }
}
