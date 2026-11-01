//! Learning a look from one example pair: our render of a photo and Lightroom's export of the
//! same photo with a preset applied. The fit finds what turns ours into theirs, in the order
//! our develop pass applies it: a 17³ colour table on the finished colour, then the vignette,
//! then grain; and it measures how close the result comes (ΔE2000).
//!
//! The table carries everything Lightroom does to colour and tone that our sliders could not
//! reproduce (its profile, curves, colour mix, grading), so it is learned on display sRGB,
//! where our LUT stage sits. Vignette and grain are spatial, which a table cannot hold, so
//! they are fitted as our own slider values.
//!
//! Pure CPU maths, like the rest of this crate: rendering our pipeline's effects is injected
//! (`render_effects`), so tests run on synthetic pairs and the app passes the real renderer.

use lf_settings::lut::Lut;

/// The table's side: 17 points per channel, a common size for looks, small enough (≈ 39 KB
/// as text) to travel inside a preset.
pub const TABLE_SIZE: usize = 17;

/// Alignment searches a pyramid of this many halvings (shifts up to about ±16 pixels).
const ALIGN_LEVELS: u32 = 4;
/// After the shift, a mean absolute luma difference above this means the pair does not show
/// the same picture (or not framed the same way).
const MISALIGNED_LUMA: f64 = 0.15;
/// The share of each edge left out: shifting, resizing and lens differences make it unreliable.
const BORDER: f64 = 0.05;
/// A channel at or beyond these is clipped: the true colour is lost, so the pixel is left out.
const CLIP_HIGH: f32 = 0.995;
const CLIP_LOW: f32 = 0.005;
/// At most this many pixels feed the table and vignette fits (evenly spread), which bounds the
/// memory of the per-cell lists at any working size.
const MAX_SAMPLES: usize = 250_000;
/// A grid point with less trilinear weight than this counts as unseen, as too few pixels
/// speak for it.
const MIN_WEIGHT: f32 = 1.0;
/// The share of each cell's pairs dropped as outliers: the furthest from the cell's median.
const OUTLIER_SHARE: f64 = 0.10;
/// Jacobi iterations of Laplacian smoothing that fill the unseen cells.
const FILL_ITERATIONS: usize = 30;
/// The light smoothing over every cell afterwards: rounds and the neighbours' weight.
const SMOOTH_ITERATIONS: usize = 2;
const SMOOTH_WEIGHT: f32 = 0.25;
/// Rounds of fitting the table and the vignette in turn.
const ROUNDS: usize = 3;
/// Grain is measured in square blocks of this side, in the flattest share of them.
const GRAIN_BLOCK: usize = 16;
const FLAT_SHARE: f64 = 0.20;
/// The grain amount of the one calibration render, and the size we keep (our default).
const CALIBRATION_GRAIN: f64 = 50.0;
const GRAIN_SIZE: f64 = 25.0;

/// An image: `w`×`h` interleaved RGB, display sRGB values in 0..1.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

impl Image {
    fn px(&self, i: usize) -> [f32; 3] {
        [self.data[i * 3], self.data[i * 3 + 1], self.data[i * 3 + 2]]
    }

    fn luma(&self) -> Vec<f32> {
        (0..self.w * self.h).map(|i| luma(self.px(i))).collect()
    }
}

/// What the fit learned, as our own settings, and how close it came.
#[derive(Clone, Debug, PartialEq)]
pub struct Fit {
    /// The colour table (size [`TABLE_SIZE`]), applied to our finished colour.
    pub table: Lut,
    /// Our vignette slider (-100..100) and its midpoint (0..100).
    pub vignette: f64,
    pub vignette_midpoint: f64,
    /// Our grain slider (0..100) and its size (kept at our default).
    pub grain: f64,
    pub grain_size: f64,
    /// ΔE2000 between our full result and Lightroom's export, over the compared pixels.
    pub delta_e_mean: f64,
    pub delta_e_p95: f64,
    /// False when the pair does not line up: a different photo, or cropped or turned
    /// differently. The numbers are still filled in, but mean little.
    pub aligned: bool,
}

/// Learns the table, vignette and grain that turn `ours` into `theirs`.
///
/// `theirs` may be any size; it is resized to `ours` and lined up by a translation. The
/// injected `render_effects` applies a fit's table, vignette and grain to our render with our
/// pipeline's maths, and returns an image the size of `ours` (another size is resized). It is
/// called twice: once at grain 50 to measure how strong our grain is, and once with the
/// result, to measure the remaining difference.
///
/// # Panics
///
/// If `ours` or `theirs` has no pixels.
pub fn fit(ours: &Image, theirs: &Image, render_effects: &mut dyn FnMut(&Fit) -> Image) -> Fit {
    let (w, h) = (ours.w, ours.h);
    assert!(
        w * h > 0 && theirs.w * theirs.h > 0,
        "an example fit needs two non-empty images"
    );
    let theirs = if (theirs.w, theirs.h) == (w, h) {
        theirs.clone()
    } else {
        resize(theirs, w, h)
    };

    // Line up: `align` gives the shift that brings `theirs` onto `ours`.
    let (dx, dy) = lf_merge::align(&rgb(ours), &rgb(&theirs), ALIGN_LEVELS);
    let theirs = shifted(&theirs, dx, dy);
    let mut mask = compared(ours, &theirs, dx, dy);
    let mut aligned = true;
    if !mask.iter().any(|&m| m) {
        // Nothing usable (all clipped, or a tiny image): compare everything, say so.
        mask = vec![true; w * h];
        aligned = false;
    }
    let (ly, ty) = (ours.luma(), theirs.luma());
    let (mut sum, mut n) = (0f64, 0usize);
    for i in (0..w * h).filter(|&i| mask[i]) {
        sum += (ly[i] - ty[i]).abs() as f64;
        n += 1;
    }
    if sum / n as f64 > MISALIGNED_LUMA {
        aligned = false;
    }

    // The pixels the table and vignette learn from, evenly thinned.
    let valid: Vec<usize> = (0..w * h).filter(|&i| mask[i]).collect();
    let stride = valid.len().div_ceil(MAX_SAMPLES).max(1);
    let samples: Vec<usize> = valid.iter().copied().step_by(stride).collect();
    let src: Vec<[f32; 3]> = samples.iter().map(|&i| ours.px(i)).collect();
    let dst: Vec<[f32; 3]> = samples.iter().map(|&i| theirs.px(i)).collect();
    let radii: Vec<f32> = samples
        .iter()
        .map(|&i| radius(i % w, i / w, w, h))
        .collect();
    let observed: Vec<f32> = dst.iter().map(|&c| luma(c)).collect();

    // Table and vignette in turn: each round learns the table from theirs with the vignette
    // taken out, then the vignette from what is left; a last table follows the last vignette.
    let (mut amount, mut mid) = (0.0, 50.0);
    let mut table = Lut::identity(TABLE_SIZE);
    for round in 0..=ROUNDS {
        let targets: Vec<[f32; 3]> = dst
            .iter()
            .zip(&radii)
            .map(|(&c, &d)| unvignette(c, d, amount, mid))
            .collect();
        table = fit_table(&src, &targets);
        if round == ROUNDS {
            break;
        }
        let predicted: Vec<f32> = src.iter().map(|&c| luma(table.apply(c))).collect();
        (amount, mid) = fit_vignette(&predicted, &observed, &radii);
    }

    let mut result = Fit {
        table,
        vignette: amount,
        vignette_midpoint: mid,
        grain: 0.0,
        grain_size: GRAIN_SIZE,
        delta_e_mean: 0.0,
        delta_e_p95: 0.0,
        aligned,
    };

    // Grain: the extra fine detail theirs has over our result without grain, in flat places,
    // turned into our slider through one calibration render.
    let predicted = Image {
        w,
        h,
        data: (0..w * h)
            .flat_map(|i| {
                let c = result.table.apply(ours.px(i));
                vignette(c, radius(i % w, i / w, w, h), amount, mid)
            })
            .collect(),
    };
    let blocks = flat_blocks(&predicted.luma(), &mask, w, h);
    if !blocks.is_empty() {
        let base = fine_detail(&predicted.luma(), w, h, &blocks);
        let theirs_fine = fine_detail(&ty, w, h, &blocks);
        let calibration = render_effects(&Fit {
            grain: CALIBRATION_GRAIN,
            ..result.clone()
        });
        let calibration = sized(calibration, w, h);
        let ours_fine = fine_detail(&calibration.luma(), w, h, &blocks);
        let per_unit = quadrature_excess(ours_fine, base) / CALIBRATION_GRAIN;
        if per_unit > 1e-6 {
            result.grain = (quadrature_excess(theirs_fine, base) / per_unit).clamp(0.0, 100.0);
        }
    }

    // How close the whole result comes, over the compared pixels.
    let finished = sized(render_effects(&result), w, h);
    let mut diffs: Vec<f64> = (0..w * h)
        .filter(|&i| mask[i])
        .map(|i| delta_e2000(finished.px(i), theirs.px(i)))
        .collect();
    diffs.sort_by(f64::total_cmp);
    result.delta_e_mean = diffs.iter().sum::<f64>() / diffs.len() as f64;
    result.delta_e_p95 = diffs[(diffs.len() * 95).div_ceil(100).max(1) - 1];
    result
}

/// `img` resized to `w`×`h` by bilinear interpolation between pixel centres.
pub fn resize(img: &Image, w: usize, h: usize) -> Image {
    let (sw, sh) = (img.w, img.h);
    let mut data = Vec::with_capacity(w * h * 3);
    let axis = |i: usize, n: usize, sn: usize| {
        let p = ((i as f32 + 0.5) * sn as f32 / n as f32 - 0.5).clamp(0.0, (sn - 1) as f32);
        let lo = (p.floor() as usize).min(sn.saturating_sub(2));
        (lo, (lo + 1).min(sn - 1), p - lo as f32)
    };
    for y in 0..h {
        let (y0, y1, ty) = axis(y, h, sh);
        for x in 0..w {
            let (x0, x1, tx) = axis(x, w, sw);
            for c in 0..3 {
                let at = |xx: usize, yy: usize| img.data[(yy * sw + xx) * 3 + c];
                let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * tx;
                let bottom = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * tx;
                data.push(top + (bottom - top) * ty);
            }
        }
    }
    Image { w, h, data }
}

/// The vignette's normalised radius at pixel (`x`, `y`) of a `w`×`h` image: 0 in the centre,
/// 1 in the corners, elliptical (following the image's shape), as the shader's
/// `length((out_uv - 0.5) * 2.0) / 1.41421356` on the pixel's centre.
pub fn radius(x: usize, y: usize, w: usize, h: usize) -> f32 {
    let u = ((x as f32 + 0.5) / w as f32 - 0.5) * 2.0;
    let v = ((y as f32 + 0.5) / h as f32 - 0.5) * 2.0;
    (u * u + v * v).sqrt() / std::f32::consts::SQRT_2
}

/// How strongly the vignette acts at normalised radius `d` for a midpoint slider (0..100):
/// the shader's `smoothstep(mix(0.05, 0.85, vignette_mid), 1.15, d)`.
fn falloff(d: f32, midpoint: f64) -> f32 {
    let start = 0.05 + 0.8 * (midpoint / 100.0) as f32;
    let t = ((d - start) / (1.15 - start)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Our vignette on colour `c` at normalised radius `d` (see [`radius`]), from the slider
/// values (`amount` -100..100, `midpoint` 0..100): a Rust copy of the develop shader's block
/// (`crates/lf-pipeline/src/shaders/develop.wgsl`, "Elliptical, following the crop's shape"),
/// whose uniforms are the sliders divided by 100 (`passes/develop.rs`). Darkening multiplies;
/// lightening mixes towards white.
pub fn vignette(c: [f32; 3], d: f32, amount: f64, midpoint: f64) -> [f32; 3] {
    if amount == 0.0 {
        return c;
    }
    let k = (amount / 100.0) as f32 * falloff(d, midpoint);
    if amount < 0.0 {
        c.map(|v| v * (1.0 + k))
    } else {
        c.map(|v| v + (1.0 - v) * k)
    }
}

/// [`vignette`] undone (as far as it can be: a full darkening leaves nothing to recover).
fn unvignette(c: [f32; 3], d: f32, amount: f64, midpoint: f64) -> [f32; 3] {
    if amount == 0.0 {
        return c;
    }
    let k = (amount / 100.0) as f32 * falloff(d, midpoint);
    if amount < 0.0 {
        let keep = (1.0 + k).max(0.05);
        c.map(|v| v / keep)
    } else {
        let keep = (1.0 - k).max(0.05);
        c.map(|v| (v - k) / keep)
    }
}

/// Fits our vignette to luma: `observed` (theirs) against `predicted` (the table on ours) at
/// each radius. For each midpoint on a 2-step grid and each direction the model is linear in
/// the amount, so it is solved exactly by least squares, with a free overall gain that soaks up
/// any brightness the table took from the vignette in an earlier round. The best fit wins.
fn fit_vignette(predicted: &[f32], observed: &[f32], radii: &[f32]) -> (f64, f64) {
    // Darkening: y = g p + (g v) p f. Lightening: y = g p + (g v) f (1 - p).
    let mut best = (f64::INFINITY, 0.0, 50.0);
    for step in 0..=50 {
        let mid = step as f64 * 2.0;
        for darken in [true, false] {
            let (mut aa, mut ab, mut bb, mut ay, mut by, mut yy) =
                (0f64, 0f64, 0f64, 0f64, 0f64, 0f64);
            for ((&p, &y), &d) in predicted.iter().zip(observed).zip(radii) {
                let f = falloff(d, mid);
                let (a, b) = if darken {
                    (p, p * f)
                } else {
                    (p, f * (1.0 - p))
                };
                let (a, b, y) = (a as f64, b as f64, y as f64);
                aa += a * a;
                ab += a * b;
                bb += b * b;
                ay += a * y;
                by += b * y;
                yy += y * y;
            }
            // Solve [aa ab; ab bb] [g; gv] = [ay; by]; fall back to no vignette when singular
            // or pointing the other way.
            let det = aa * bb - ab * ab;
            let mut amount = 0.0;
            if det.abs() > 1e-12 * aa.max(1.0) * bb.max(1e-12) {
                let g = (ay * bb - by * ab) / det;
                let gv = (aa * by - ab * ay) / det;
                if g > 1e-9 {
                    let v = (gv / g * 100.0).clamp(-100.0, 100.0);
                    if (v < 0.0) == darken {
                        amount = v;
                    }
                }
            }
            // The residual with the amount fixed and the gain refitted.
            let k = amount / 100.0;
            let (sx, sxy) = (aa + 2.0 * k * ab + k * k * bb, ay + k * by);
            let sse = if sx > 0.0 { yy - sxy * sxy / sx } else { yy };
            if sse < best.0 - 1e-12 {
                best = (sse, amount, mid);
            }
        }
    }
    let (_, amount, mid) = best;
    if amount.abs() < 0.5 {
        (0.0, 50.0)
    } else {
        (amount, mid)
    }
}

/// Learns a [`TABLE_SIZE`]³ table mapping `src` colours to `dst` colours.
///
/// Each pair spreads over the eight grid points around its `src` colour by trilinear weight.
/// Each point keeps the weighted mean of its pairs' *changes* (`dst - src`), after dropping
/// the [`OUTLIER_SHARE`] furthest from the point's median change: learning the change rather
/// than the colour keeps a point fed only from one side (the edge of what the photo holds)
/// from being pulled towards its neighbours' colours. Unseen points start from the nearest
/// seen point's change and are then filled by Laplacian smoothing with the seen ones pinned,
/// so colours the example never showed get a smooth continuation of the look rather than
/// garbage; a light smoothing over every point then takes out the noise.
fn fit_table(src: &[[f32; 3]], dst: &[[f32; 3]]) -> Lut {
    let n = TABLE_SIZE;
    let cells = n * n * n;
    let index = |r: usize, g: usize, b: usize| (b * n + g) * n + r;
    let mut lists: Vec<Vec<(u32, f32)>> = vec![Vec::new(); cells];
    for (k, c) in src.iter().enumerate() {
        let pos = c.map(|v| v.clamp(0.0, 1.0) * (n - 1) as f32);
        let lo = pos.map(|p| (p.floor() as usize).min(n - 2));
        let t = [0, 1, 2].map(|i| pos[i] - lo[i] as f32);
        for corner in 0..8 {
            let bit = |i: usize| (corner >> i) & 1;
            let weight = (0..3)
                .map(|i| if bit(i) == 1 { t[i] } else { 1.0 - t[i] })
                .product::<f32>();
            if weight > 1e-6 {
                lists[index(lo[0] + bit(0), lo[1] + bit(1), lo[2] + bit(2))]
                    .push((k as u32, weight));
            }
        }
    }

    let mut change = vec![[0f32; 3]; cells];
    let mut seen = vec![false; cells];
    for (cell, list) in lists.iter_mut().enumerate() {
        if list.iter().map(|e| e.1).sum::<f32>() < MIN_WEIGHT {
            continue;
        }
        let delta = |k: u32| {
            let (s, d) = (src[k as usize], dst[k as usize]);
            [d[0] - s[0], d[1] - s[1], d[2] - s[2]]
        };
        let median = [0, 1, 2].map(|c| {
            let mut v: Vec<f32> = list.iter().map(|e| delta(e.0)[c]).collect();
            let mid = v.len() / 2;
            *v.select_nth_unstable_by(mid, f32::total_cmp).1
        });
        let distance = |k: u32| {
            let d = delta(k);
            (0..3).map(|c| (d[c] - median[c]).powi(2)).sum::<f32>()
        };
        list.sort_by(|a, b| distance(a.0).total_cmp(&distance(b.0)));
        let keep = list.len() - (list.len() as f64 * OUTLIER_SHARE) as usize;
        let (mut sum, mut total) = ([0f32; 3], 0f32);
        for &(k, weight) in &list[..keep] {
            let d = delta(k);
            for c in 0..3 {
                sum[c] += d[c] * weight;
            }
            total += weight;
        }
        if total > 0.0 {
            change[cell] = sum.map(|s| s / total);
            seen[cell] = true;
        }
    }

    // The mean of a point's neighbours (six, fewer on the cube's faces).
    let neighbours = |field: &[[f32; 3]], cell: usize| {
        let mut sum = [0f32; 3];
        let mut count = 0f32;
        for i in adjacent(cell, n) {
            for c in 0..3 {
                sum[c] += field[i][c];
            }
            count += 1.0;
        }
        sum.map(|s| s / count)
    };
    // Unseen points start from the nearest seen point's change (breadth first over the grid),
    // so colours beyond what the example showed carry on its look rather than drop back to
    // none; the smoothing then blends them.
    let mut queue: std::collections::VecDeque<usize> = (0..cells).filter(|&c| seen[c]).collect();
    let mut reached = seen.clone();
    while let Some(cell) = queue.pop_front() {
        for next in adjacent(cell, n) {
            if !reached[next] {
                reached[next] = true;
                change[next] = change[cell];
                queue.push_back(next);
            }
        }
    }
    for _ in 0..FILL_ITERATIONS {
        let before = change.clone();
        for cell in (0..cells).filter(|&c| !seen[c]) {
            change[cell] = neighbours(&before, cell);
        }
    }
    for _ in 0..SMOOTH_ITERATIONS {
        let before = change.clone();
        for (cell, out) in change.iter_mut().enumerate() {
            let around = neighbours(&before, cell);
            for c in 0..3 {
                out[c] = before[cell][c] * (1.0 - SMOOTH_WEIGHT) + around[c] * SMOOTH_WEIGHT;
            }
        }
    }

    let mut table = Lut::identity(n);
    for (entry, d) in table.data.iter_mut().zip(&change) {
        for c in 0..3 {
            entry[c] = (entry[c] + d[c]).clamp(0.0, 1.0);
        }
    }
    table
}

/// The grid points next to `cell` in a side-`n` table (six, fewer on the cube's faces).
fn adjacent(cell: usize, n: usize) -> impl Iterator<Item = usize> {
    let p = [cell % n, (cell / n) % n, cell / (n * n)];
    (0..3).flat_map(move |axis| {
        let stride = n.pow(axis as u32);
        let down = (p[axis] > 0).then(|| cell - stride);
        let up = (p[axis] + 1 < n).then(|| cell + stride);
        down.into_iter().chain(up)
    })
}

/// The top-left corners of the [`FLAT_SHARE`] flattest [`GRAIN_BLOCK`]-pixel blocks lying
/// wholly in the compared pixels, by the spread of `luma` inside each.
fn flat_blocks(luma: &[f32], mask: &[bool], w: usize, h: usize) -> Vec<(usize, usize)> {
    let mut blocks = Vec::new();
    for by in (0..h.saturating_sub(GRAIN_BLOCK - 1)).step_by(GRAIN_BLOCK) {
        for bx in (0..w.saturating_sub(GRAIN_BLOCK - 1)).step_by(GRAIN_BLOCK) {
            let pixels = || {
                (by..by + GRAIN_BLOCK)
                    .flat_map(move |y| (bx..bx + GRAIN_BLOCK).map(move |x| y * w + x))
            };
            if !pixels().all(|i| mask[i]) {
                continue;
            }
            let count = (GRAIN_BLOCK * GRAIN_BLOCK) as f64;
            let mean = pixels().map(|i| luma[i] as f64).sum::<f64>() / count;
            let var = pixels()
                .map(|i| (luma[i] as f64 - mean).powi(2))
                .sum::<f64>()
                / count;
            blocks.push((var, bx, by));
        }
    }
    blocks.sort_by(|a, b| a.0.total_cmp(&b.0));
    let keep = ((blocks.len() as f64 * FLAT_SHARE).ceil() as usize).min(blocks.len());
    blocks[..keep].iter().map(|&(_, x, y)| (x, y)).collect()
}

/// The standard deviation of `luma`'s fine detail (each pixel minus its 3×3 box mean) over
/// the pixels of `blocks`.
fn fine_detail(luma: &[f32], w: usize, h: usize, blocks: &[(usize, usize)]) -> f64 {
    let at = |x: isize, y: isize| {
        luma[(y.clamp(0, h as isize - 1) as usize) * w + x.clamp(0, w as isize - 1) as usize]
    };
    let (mut sum, mut sq, mut count) = (0f64, 0f64, 0f64);
    for &(bx, by) in blocks {
        for y in by..by + GRAIN_BLOCK {
            for x in bx..bx + GRAIN_BLOCK {
                let (xi, yi) = (x as isize, y as isize);
                let mut box3 = 0f32;
                for oy in -1..=1 {
                    for ox in -1..=1 {
                        box3 += at(xi + ox, yi + oy);
                    }
                }
                let v = (luma[y * w + x] - box3 / 9.0) as f64;
                sum += v;
                sq += v * v;
                count += 1.0;
            }
        }
    }
    let mean = sum / count;
    (sq / count - mean * mean).max(0.0).sqrt()
}

/// How much of `with` is beyond `without`, for independent noise (`sqrt(a² - b²)`, never
/// below zero).
fn quadrature_excess(with: f64, without: f64) -> f64 {
    (with * with - without * without).max(0.0).sqrt()
}

/// Which pixels are compared: inside the 5 % border, still inside `theirs` after the shift,
/// and clipped in neither image.
fn compared(ours: &Image, theirs: &Image, dx: i32, dy: i32) -> Vec<bool> {
    let (w, h) = (ours.w, ours.h);
    let (bx, by) = (
        (w as f64 * BORDER).ceil() as usize,
        (h as f64 * BORDER).ceil() as usize,
    );
    let clipped = |c: [f32; 3]| c.iter().any(|&v| !(CLIP_LOW..CLIP_HIGH).contains(&v));
    (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let (sx, sy) = (x as i64 - dx as i64, y as i64 - dy as i64);
            x >= bx
                && x < w.saturating_sub(bx)
                && y >= by
                && y < h.saturating_sub(by)
                && (0..w as i64).contains(&sx)
                && (0..h as i64).contains(&sy)
                && !clipped(ours.px(i))
                && !clipped(theirs.px(i))
        })
        .collect()
}

fn rgb(img: &Image) -> lf_merge::Rgb {
    lf_merge::Rgb {
        w: img.w,
        h: img.h,
        data: img.data.clone(),
    }
}

fn shifted(img: &Image, dx: i32, dy: i32) -> Image {
    let out = rgb(img).shifted(dx, dy);
    Image {
        w: out.w,
        h: out.h,
        data: out.data,
    }
}

fn sized(img: Image, w: usize, h: usize) -> Image {
    if (img.w, img.h) == (w, h) {
        img
    } else {
        resize(&img, w, h)
    }
}

/// Luma on gamma values, as the shader's.
fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// The CIEDE2000 colour difference between two display sRGB colours (0..1, D65).
pub fn delta_e2000(a: [f32; 3], b: [f32; 3]) -> f64 {
    delta_e2000_lab(lab(a), lab(b))
}

/// CIE L*a*b* (D65 white) of a display sRGB colour.
fn lab(c: [f32; 3]) -> [f64; 3] {
    let lin = c.map(|v| {
        let v = (v as f64).clamp(0.0, 1.0);
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    });
    let [r, g, b] = lin;
    let x = 0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b;
    let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175_0 * b;
    let z = 0.019_333_9 * r + 0.119_192_0 * g + 0.950_304_1 * b;
    let f = |t: f64| {
        if t > 216.0 / 24389.0 {
            t.cbrt()
        } else {
            (24389.0 / 27.0 * t + 16.0) / 116.0
        }
    };
    let (fx, fy, fz) = (f(x / 0.950_47), f(y), f(z / 1.088_83));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// The CIEDE2000 difference between two L*a*b* colours (kL = kC = kH = 1), following Sharma,
/// Wu and Dalal (2005), whose published test pairs check it.
pub fn delta_e2000_lab(lab1: [f64; 3], lab2: [f64; 3]) -> f64 {
    let [l1, a1, b1] = lab1;
    let [l2, a2, b2] = lab2;
    let pow7 = |v: f64| v.powi(7);
    let c_bar = (a1.hypot(b1) + a2.hypot(b2)) / 2.0;
    let g = 0.5 * (1.0 - (pow7(c_bar) / (pow7(c_bar) + pow7(25.0))).sqrt());
    let (a1p, a2p) = ((1.0 + g) * a1, (1.0 + g) * a2);
    let (c1p, c2p) = (a1p.hypot(b1), a2p.hypot(b2));
    let hue = |b: f64, a: f64| {
        if a == 0.0 && b == 0.0 {
            0.0
        } else {
            b.atan2(a).to_degrees().rem_euclid(360.0)
        }
    };
    let (h1p, h2p) = (hue(b1, a1p), hue(b2, a2p));

    let dl = l2 - l1;
    let dc = c2p - c1p;
    let dh = if c1p * c2p == 0.0 {
        0.0
    } else {
        let d = h2p - h1p;
        if d > 180.0 {
            d - 360.0
        } else if d < -180.0 {
            d + 360.0
        } else {
            d
        }
    };
    let dh_big = 2.0 * (c1p * c2p).sqrt() * (dh / 2.0).to_radians().sin();

    let l_bar = (l1 + l2) / 2.0;
    let c_bar_p = (c1p + c2p) / 2.0;
    let h_bar = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        (h1p + h2p) / 2.0
    } else if h1p + h2p < 360.0 {
        (h1p + h2p + 360.0) / 2.0
    } else {
        (h1p + h2p - 360.0) / 2.0
    };
    let cos = |deg: f64| deg.to_radians().cos();
    let t =
        1.0 - 0.17 * cos(h_bar - 30.0) + 0.24 * cos(2.0 * h_bar) + 0.32 * cos(3.0 * h_bar + 6.0)
            - 0.20 * cos(4.0 * h_bar - 63.0);
    let d_theta = 30.0 * (-((h_bar - 275.0) / 25.0).powi(2)).exp();
    let rc = 2.0 * (pow7(c_bar_p) / (pow7(c_bar_p) + pow7(25.0))).sqrt();
    let sl = 1.0 + 0.015 * (l_bar - 50.0).powi(2) / (20.0 + (l_bar - 50.0).powi(2)).sqrt();
    let sc = 1.0 + 0.045 * c_bar_p;
    let sh = 1.0 + 0.015 * c_bar_p * t;
    let rt = -(2.0 * d_theta).to_radians().sin() * rc;
    let (tl, tc, th) = (dl / sl, dc / sc, dh_big / sh);
    (tl * tl + tc * tc + th * th + rt * tc * th).sqrt()
}
