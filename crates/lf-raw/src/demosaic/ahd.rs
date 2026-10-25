//! Adaptive Homogeneity-Directed (AHD) Bayer demosaic.
//!
//! Source: K. Hirakawa and T. W. Parks, "Adaptive homogeneity-directed
//! demosaicing algorithm", IEEE Transactions on Image Processing 14(3), 2005,
//! pp. 360-369. Written from the paper's description; no code from dcraw,
//! darktable, RawTherapee or other GPL implementations was used.
//!
//! Steps, each producing the input of the next:
//!
//! 1. **Directional green.** Two full green planes, one interpolated along rows
//!    and one along columns. At a red or blue site the horizontal estimate is the
//!    5-tap filter `[-1/4, 1/2, 1/2, 1/2, -1/4]` over the mosaic row (the mean of the
//!    two green neighbours plus a Laplacian correction from the site's own colour),
//!    limited to the range of those two greens so it cannot overshoot; vertical
//!    likewise down the column. The range limit is not in the paper: it is a
//!    generic overshoot limiter added here, not taken from any implementation.
//! 2. **Red and blue from colour differences.** For each direction, a missing red
//!    (blue) value is that direction's green plus the mean of `R - G` (`B - G`)
//!    over the neighbouring red (blue) sites in the 3x3 window, using the same
//!    direction's green at those sites. This gives two full RGB candidates.
//! 3. **CIELab.** Both candidates are converted to CIELab, treating camera RGB as
//!    linear sRGB (fixed sRGB-to-XYZ D65 matrix). The test below only compares
//!    relative distances, so the approximate primaries do not matter.
//! 4. **Homogeneity maps.** For each pixel, the paper's adaptive tolerances are
//!    `eps_L = min(max |dL| to the left/right neighbours in the horizontal image,
//!    max |dL| to the up/down neighbours in the vertical image)` and the same for
//!    the chroma distance `eps_C` (distance in the a*b* plane). A direction's
//!    homogeneity at the pixel is the number of its 4 neighbours (ball of radius 1)
//!    whose luminance and chroma distances are both within the tolerances.
//! 5. **Direction selection.** The homogeneity values are summed over the 3x3
//!    window; the direction with the larger sum supplies the pixel (ties average
//!    the two candidates).
//! 6. **Artefact reduction.** One pass of the paper's median post-filter: 3x3
//!    medians of `R - G` and `B - G`; the measured sample is kept and the other two
//!    channels rebuilt from it and the medians.
//!
//! The frame is processed in horizontal stripes in parallel; each stripe reads a
//! margin of rows and columns (mirrored at the frame edges) wide enough for all
//! six steps, so the stripes are independent and the result matches a whole-frame
//! pass.

use super::{Plane, mirror};
use crate::Rect;
use rayon::prelude::*;

/// Rows per stripe.
const STRIPE: usize = 32;
/// Margin around a stripe: green 2 + colour 1 + homogeneity 1 + 3x3 sum 1 + median 1.
const MARGIN: usize = 8;

pub(crate) fn run(p: &Plane, pattern: [[usize; 2]; 2], crop: Rect) -> Vec<f32> {
    let cw = crop.w as usize;
    let mut out = vec![0.0f32; cw * crop.h as usize * 3];
    if cw == 0 {
        return out;
    }
    out.par_chunks_mut(cw * 3 * STRIPE)
        .enumerate()
        .for_each(|(i, chunk)| {
            let rows = chunk.len() / (cw * 3);
            let x0 = crop.x as isize;
            let y0 = (crop.y as usize + i * STRIPE) as isize;
            Stripe::new(p, pattern, x0, y0, cw, rows).develop(chunk);
        });
    out
}

/// A stripe's working window: the output rows plus `MARGIN` on every side.
struct Stripe {
    w: usize,
    h: usize,
    /// Output size inside the margin.
    cw: usize,
    rows: usize,
    /// CFA colour (0 R, 1 G, 2 B) per window pixel.
    colour: Vec<u8>,
    /// Normalised mosaic samples per window pixel.
    m: Vec<f32>,
}

/// CIELab of one candidate pixel.
#[derive(Clone, Copy, Default)]
struct Lab {
    l: f32,
    a: f32,
    b: f32,
}

impl Stripe {
    fn new(
        p: &Plane,
        pattern: [[usize; 2]; 2],
        x0: isize,
        y0: isize,
        cw: usize,
        rows: usize,
    ) -> Stripe {
        let (w, h) = (cw + 2 * MARGIN, rows + 2 * MARGIN);
        let mut colour = Vec::with_capacity(w * h);
        let mut m = Vec::with_capacity(w * h);
        for ly in 0..h {
            let gy = y0 + ly as isize - MARGIN as isize;
            let row = mirror(gy, p.h) * p.w;
            for lx in 0..w {
                let gx = x0 + lx as isize - MARGIN as isize;
                // Mirroring keeps parity, so the colour follows the unmirrored coordinate.
                colour.push(pattern[(gy & 1) as usize][(gx & 1) as usize] as u8);
                m.push(p.data[row + mirror(gx, p.w)]);
            }
        }
        Stripe {
            w,
            h,
            cw,
            rows,
            colour,
            m,
        }
    }

    /// Run steps 1-6 and write the stripe's output rows into `out`.
    fn develop(&self, out: &mut [f32]) {
        let (gh, gv) = self.directional_green(2);
        let rgb_h = self.colour_from_differences(&gh, 3);
        let rgb_v = self.colour_from_differences(&gv, 3);
        let lab_h = self.to_lab(&rgb_h, 3);
        let lab_v = self.to_lab(&rgb_v, 3);
        let (hom_h, hom_v) = self.homogeneity(&lab_h, &lab_v, 4);
        let chosen = self.select(&rgb_h, &rgb_v, &hom_h, &hom_v, 5);
        self.median_refine(&chosen, out);
    }

    /// Iterate `f(index)` over the window pixels at least `inset` from its edges.
    fn each(&self, inset: usize, mut f: impl FnMut(usize)) {
        for y in inset..self.h - inset {
            for x in inset..self.w - inset {
                f(y * self.w + x);
            }
        }
    }

    /// Step 1: horizontal and vertical green planes.
    fn directional_green(&self, inset: usize) -> (Vec<f32>, Vec<f32>) {
        let (w, m) = (self.w, &self.m);
        let mut gh = vec![0.0f32; m.len()];
        let mut gv = vec![0.0f32; m.len()];
        let estimate = |i: usize, step: usize| {
            let (a, b) = (m[i - step], m[i + step]);
            let v = (a + b) * 0.5 + (2.0 * m[i] - m[i - 2 * step] - m[i + 2 * step]) * 0.25;
            v.clamp(a.min(b), a.max(b))
        };
        self.each(inset, |i| {
            if self.colour[i] == 1 {
                gh[i] = m[i];
                gv[i] = m[i];
            } else {
                gh[i] = estimate(i, 1);
                gv[i] = estimate(i, w);
            }
        });
        (gh, gv)
    }

    /// Step 2: full RGB for one direction from its green plane.
    fn colour_from_differences(&self, g: &[f32], inset: usize) -> Vec<[f32; 3]> {
        let w = self.w as isize;
        let mut rgb = vec![[0.0f32; 3]; self.m.len()];
        self.each(inset, |i| {
            let own = self.colour[i] as usize;
            let mut px = [g[i]; 3];
            px[own] = self.m[i];
            let mut sum = [0.0f32; 3];
            let mut n = [0u32; 3];
            for dy in -1..=1isize {
                for dx in -1..=1isize {
                    let j = (i as isize + dy * w + dx) as usize;
                    let c = self.colour[j] as usize;
                    sum[c] += self.m[j] - g[j];
                    n[c] += 1;
                }
            }
            for c in [0, 2] {
                if c != own {
                    px[c] = g[i] + sum[c] / n[c] as f32;
                }
            }
            rgb[i] = px;
        });
        rgb
    }

    /// Step 3: CIELab of one candidate image.
    fn to_lab(&self, rgb: &[[f32; 3]], inset: usize) -> Vec<Lab> {
        let mut lab = vec![Lab::default(); rgb.len()];
        self.each(inset, |i| lab[i] = lab_of(rgb[i]));
        lab
    }

    /// Step 4: homogeneity maps (0..=4 per pixel) for both directions.
    fn homogeneity(&self, lh: &[Lab], lv: &[Lab], inset: usize) -> (Vec<u8>, Vec<u8>) {
        let w = self.w;
        let mut hh = vec![0u8; lh.len()];
        let mut hv = vec![0u8; lh.len()];
        let dl = |a: Lab, b: Lab| (a.l - b.l).abs();
        let dc2 = |a: Lab, b: Lab| (a.a - b.a).powi(2) + (a.b - b.b).powi(2);
        self.each(inset, |i| {
            let eps_l = (dl(lh[i], lh[i - 1]).max(dl(lh[i], lh[i + 1])))
                .min(dl(lv[i], lv[i - w]).max(dl(lv[i], lv[i + w])));
            let eps_c2 = (dc2(lh[i], lh[i - 1]).max(dc2(lh[i], lh[i + 1])))
                .min(dc2(lv[i], lv[i - w]).max(dc2(lv[i], lv[i + w])));
            let count = |lab: &[Lab]| {
                [i - 1, i + 1, i - w, i + w]
                    .into_iter()
                    .filter(|&j| dl(lab[i], lab[j]) <= eps_l && dc2(lab[i], lab[j]) <= eps_c2)
                    .count() as u8
            };
            hh[i] = count(lh);
            hv[i] = count(lv);
        });
        (hh, hv)
    }

    /// Step 5: pick the more homogeneous direction per pixel (3x3 sums).
    fn select(
        &self,
        rgb_h: &[[f32; 3]],
        rgb_v: &[[f32; 3]],
        hom_h: &[u8],
        hom_v: &[u8],
        inset: usize,
    ) -> Vec<[f32; 3]> {
        let w = self.w;
        let mut out = vec![[0.0f32; 3]; rgb_h.len()];
        self.each(inset, |i| {
            let (mut sh, mut sv) = (0u32, 0u32);
            for row in [i - w, i, i + w] {
                for j in row - 1..=row + 1 {
                    sh += hom_h[j] as u32;
                    sv += hom_v[j] as u32;
                }
            }
            out[i] = match sh.cmp(&sv) {
                std::cmp::Ordering::Greater => rgb_h[i],
                std::cmp::Ordering::Less => rgb_v[i],
                std::cmp::Ordering::Equal => {
                    let (a, b) = (rgb_h[i], rgb_v[i]);
                    [
                        (a[0] + b[0]) * 0.5,
                        (a[1] + b[1]) * 0.5,
                        (a[2] + b[2]) * 0.5,
                    ]
                }
            };
        });
        out
    }

    /// Step 6: 3x3 median of the colour differences, then rebuild around the
    /// measured sample. Writes the stripe's core (inside the margin) to `out`.
    fn median_refine(&self, rgb: &[[f32; 3]], out: &mut [f32]) {
        let w = self.w;
        let median = |i: usize, c: usize| {
            let mut v = [0.0f32; 9];
            let mut k = 0;
            for row in [i - w, i, i + w] {
                for px in &rgb[row - 1..=row + 1] {
                    v[k] = px[c] - px[1];
                    k += 1;
                }
            }
            *v.select_nth_unstable_by(4, f32::total_cmp).1
        };
        for y in 0..self.rows {
            for x in 0..self.cw {
                let i = (y + MARGIN) * w + x + MARGIN;
                let (dr, db) = (median(i, 0), median(i, 2));
                let s = self.m[i];
                let px = match self.colour[i] {
                    0 => [s, s - dr, s - dr + db],
                    2 => [s - db + dr, s - db, s],
                    _ => [s + dr, s, s + db],
                };
                out[(y * self.cw + x) * 3..][..3].copy_from_slice(&px);
            }
        }
    }
}

/// Linear RGB (treated as sRGB primaries, D65) to CIELab.
fn lab_of([r, g, b]: [f32; 3]) -> Lab {
    let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.950_47;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.088_83;
    let f = |t: f32| {
        if t > 0.008_856 {
            t.cbrt()
        } else {
            7.787 * t + 16.0 / 116.0
        }
    };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    Lab {
        l: 116.0 * fy - 16.0,
        a: 500.0 * (fx - fy),
        b: 200.0 * (fy - fz),
    }
}
