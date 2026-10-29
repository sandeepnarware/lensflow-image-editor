//! Lens blur: a shallow depth of field made after the fact, from a depth map
//! ([`crate::depth`]). The focus is the subject's distance (by default the nearest part of the
//! middle of the frame, [`auto_focus`]); things within the focus range stay sharp, and the
//! blur grows with distance from it ([`coc`], the circle of confusion), up to the amount
//! asked.
//!
//! The blur is a disc (a lens's bokeh, not a Gaussian), worked in linear light so bright
//! points bloom as they do through a lens. It is made as a few layers of growing radius
//! ([`blur`]): each pixel mixes the two layers either side of its own circle. Within a layer,
//! each sample counts as much as it is itself out of focus, so a sharp subject does not
//! smear into the blurred background behind it (the halo a plain blur leaves).

use crate::{Gray, Rgb};
use rayon::prelude::*;

/// How many blur layers (more: smoother steps, slower).
const LAYERS: usize = 6;

/// The focus depth for `depth` (0 far .. 1 near): the near end (90th percentile) of the middle
/// half of the frame, where a subject usually is.
pub fn auto_focus(depth: &Gray) -> f32 {
    let (x0, x1) = (depth.w / 4, depth.w * 3 / 4);
    let (y0, y1) = (depth.h / 4, depth.h * 3 / 4);
    let mut v: Vec<f32> = (y0..y1.max(y0 + 1).min(depth.h))
        .flat_map(|y| (x0..x1.max(x0 + 1).min(depth.w)).map(move |x| (x, y)))
        .map(|(x, y)| depth.data[y * depth.w + x])
        .collect();
    if v.is_empty() {
        return 1.0;
    }
    v.sort_by(f32::total_cmp);
    v[(v.len() - 1) * 9 / 10]
}

/// How out of focus a pixel at depth `d` is, 0 (sharp) .. 1 (fully blurred), for `focus` and
/// a focus `range` (0..1: how deep the sharp zone is).
pub fn coc(d: f32, focus: f32, range: f32) -> f32 {
    let half = (range * 0.5).max(0.0);
    let off = ((d - focus).abs() - half).max(0.0);
    // Full blur a third of the way across the depth range past the sharp zone.
    (off / 0.33).clamp(0.0, 1.0)
}

/// A disc blur of radius `r` (pixels) of the planar channels in `c` (each `w × h`), each
/// sample weighted by `wt`; returns the weighted means. Rows of the disc are read through
/// running sums, so the cost is the radius, not its square.
fn disc(c: &[Vec<f32>], wt: &[f32], w: usize, h: usize, r: usize) -> Vec<Vec<f32>> {
    // Row prefix sums of weight × channel, and of weight.
    let prefix = |v: &dyn Fn(usize) -> f32| -> Vec<f32> {
        let mut p = vec![0.0f32; h * (w + 1)];
        for y in 0..h {
            let mut s = 0.0;
            for x in 0..w {
                s += v(y * w + x);
                p[y * (w + 1) + x + 1] = s;
            }
        }
        p
    };
    let pw = prefix(&|i| wt[i]);
    let pc: Vec<Vec<f32>> = c.iter().map(|ch| prefix(&|i| ch[i] * wt[i])).collect();
    let half: Vec<usize> = (0..=r)
        .map(|dy| (((r * r - dy * dy) as f64).sqrt()) as usize)
        .collect();
    let rows: Vec<Vec<f32>> = (0..h)
        .into_par_iter()
        .map(|y| {
            let mut out = vec![0.0f32; w * (c.len() + 1)];
            for x in 0..w {
                let (mut sw, mut sc) = (0.0f32, vec![0.0f32; c.len()]);
                for dy in -(r as isize)..=(r as isize) {
                    let yy = (y as isize + dy).clamp(0, h as isize - 1) as usize;
                    let hw = half[dy.unsigned_abs()];
                    let a = x.saturating_sub(hw);
                    let b = (x + hw + 1).min(w);
                    let row = yy * (w + 1);
                    sw += pw[row + b] - pw[row + a];
                    for (k, p) in pc.iter().enumerate() {
                        sc[k] += p[row + b] - p[row + a];
                    }
                }
                out[x * (c.len() + 1)] = sw;
                for k in 0..c.len() {
                    out[x * (c.len() + 1) + 1 + k] = sc[k];
                }
            }
            out
        })
        .collect();
    let mut res = vec![vec![0.0f32; w * h]; c.len()];
    for (y, row) in rows.iter().enumerate() {
        for x in 0..w {
            let sw = row[x * (c.len() + 1)].max(1e-6);
            for (k, ch) in res.iter_mut().enumerate() {
                ch[y * w + x] = row[x * (c.len() + 1) + 1 + k] / sw;
            }
        }
    }
    res
}

/// `img` with each pixel blurred by its circle of confusion `coc` (0..1, `img`'s size),
/// `max_radius` pixels at 1.
pub fn blur(img: &Rgb, coc: &[f32], max_radius: f32) -> Rgb {
    let (w, h) = (img.w, img.h);
    let n = w * h;
    // Linear light, planar.
    let lin: Vec<Vec<f32>> = (0..3)
        .map(|c| {
            (0..n)
                .map(|i| img.data[i * 3 + c].max(0.0).powf(2.2))
                .collect()
        })
        .collect();
    if max_radius < 0.5 || coc.iter().all(|&v| v <= 0.0) {
        return img.clone();
    }
    // A sample's own blur is its weight (a little for sharp ones, so a disc never empties).
    let wt: Vec<f32> = coc.iter().map(|&v| 0.02 + v).collect();
    // Each pixel mixes the layers either side of its circle: layer k0 by (1 − f), k0 + 1 by
    // f. Layers are made one at a time and added in, so only one is held.
    let place = |i: usize| -> (usize, f32) {
        let t = coc[i].clamp(0.0, 1.0) * LAYERS as f32;
        if t >= LAYERS as f32 {
            (LAYERS - 1, 1.0)
        } else {
            (t.floor() as usize, t - t.floor())
        }
    };
    let mut acc = vec![vec![0.0f32; n]; 3];
    for k in 0..=LAYERS {
        let r = max_radius * k as f32 / LAYERS as f32;
        let layer = if r < 0.5 {
            lin.clone()
        } else {
            layer_at(&lin, &wt, w, h, r)
        };
        for i in 0..n {
            let (k0, f) = place(i);
            let share = if k == k0 {
                1.0 - f
            } else if k == k0 + 1 {
                f
            } else {
                continue;
            };
            for c in 0..3 {
                acc[c][i] += layer[c][i] * share;
            }
        }
    }
    let mut out = Rgb::new(w, h);
    for (i, px) in out.data.as_chunks_mut::<3>().0.iter_mut().enumerate() {
        for (v, ch) in px.iter_mut().zip(&acc) {
            *v = ch[i].max(0.0).powf(1.0 / 2.2);
        }
    }
    out
}

/// The largest disc radius blurred at full size; a larger one is blurred on a picture made
/// smaller by as much and brought back (a blur that wide loses nothing by it).
const DIRECT_RADIUS: f32 = 12.0;

/// One layer: the weighted disc blur of radius `r` (pixels at `w × h`).
fn layer_at(lin: &[Vec<f32>], wt: &[f32], w: usize, h: usize, r: f32) -> Vec<Vec<f32>> {
    if r <= DIRECT_RADIUS {
        return disc(lin, wt, w, h, r.round() as usize);
    }
    let k = DIRECT_RADIUS / r;
    let (sw, sh) = (
        ((w as f32 * k).round() as usize).max(1),
        ((h as f32 * k).round() as usize).max(1),
    );
    let small = |v: &[f32]| {
        Gray {
            w,
            h,
            data: v.to_vec(),
        }
        .resized(sw, sh)
        .data
    };
    // Weighted channels go down as (value × weight) and weight, so the weighting survives.
    let small_wt = small(wt);
    let small_lin: Vec<Vec<f32>> = lin
        .iter()
        .map(|ch| {
            let weighted: Vec<f32> = ch.iter().zip(wt).map(|(v, w)| v * w).collect();
            small(&weighted)
                .iter()
                .zip(&small_wt)
                .map(|(v, w)| v / w.max(1e-6))
                .collect()
        })
        .collect();
    disc(&small_lin, &small_wt, sw, sh, DIRECT_RADIUS as usize)
        .into_iter()
        .map(|ch| {
            Gray {
                w: sw,
                h: sh,
                data: ch,
            }
            .resized(w, h)
            .data
        })
        .collect()
}

/// `img` with a lens blur from `depth` (0 far .. 1 near, `img`'s size): `amount` 0..1 sets
/// the largest blur (up to 3 % of the longer side), `range` how deep the sharp zone is, and
/// `focus` the depth in focus (`None`: [`auto_focus`]).
pub fn lens_blur(img: &Rgb, depth: &Gray, amount: f32, range: f32, focus: Option<f32>) -> Rgb {
    let focus = focus.unwrap_or_else(|| auto_focus(depth));
    let c: Vec<f32> = depth.data.iter().map(|&d| coc(d, focus, range)).collect();
    let max_radius = amount.clamp(0.0, 1.0) * 0.03 * img.w.max(img.h) as f32;
    blur(img, &c, max_radius)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_focus_zone_is_sharp_and_blur_grows_past_it() {
        assert_eq!(coc(0.8, 0.8, 0.2), 0.0);
        assert_eq!(coc(0.75, 0.8, 0.2), 0.0, "within the range");
        assert!(coc(0.5, 0.8, 0.2) > 0.5);
        assert_eq!(coc(0.0, 0.8, 0.2), 1.0);
        assert!(coc(0.5, 0.8, 0.0) > coc(0.5, 0.8, 0.4));
    }

    #[test]
    fn auto_focus_finds_the_near_subject_in_the_middle() {
        let (w, h) = (40, 40);
        let mut d = Gray {
            w,
            h,
            data: vec![0.1; w * h],
        };
        for y in 15..25 {
            for x in 15..25 {
                d.data[y * w + x] = 0.9;
            }
        }
        assert!(auto_focus(&d) > 0.8);
    }

    #[test]
    fn a_point_of_light_spreads_into_an_even_disc() {
        let (w, h) = (41, 41);
        let lin = vec![
            (0..w * h)
                .map(|i| if i == 20 * w + 20 { 1.0 } else { 0.0 })
                .collect::<Vec<f32>>(),
        ];
        let wt = vec![1.0; w * h];
        let out = &disc(&lin, &wt, w, h, 5)[0];
        let centre = out[20 * w + 20];
        assert!(centre > 0.0);
        // Even inside the disc, nothing outside it.
        assert!((out[20 * w + 23] - centre).abs() < 1e-6);
        assert!((out[23 * w + 20] - centre).abs() < 1e-6);
        assert_eq!(out[20 * w + 27], 0.0);
        assert_eq!(out[10 * w + 10], 0.0);
    }

    #[test]
    fn a_sharp_subject_does_not_halo_into_the_background() {
        // A bright sharp square on a dark blurred ground.
        let (w, h) = (60, 60);
        let mut img = Rgb::new(w, h);
        let mut c = vec![1.0f32; w * h];
        for y in 20..40 {
            for x in 20..40 {
                img.data[(y * w + x) * 3..(y * w + x) * 3 + 3].fill(1.0);
                c[y * w + x] = 0.0;
            }
        }
        let out = blur(&img, &c, 8.0);
        // The subject stays as it was.
        assert_eq!(out.at(30, 30), [1.0, 1.0, 1.0]);
        // Just outside it, the ground stays dark (weighted: the sharp subject barely counts).
        assert!(out.at(43, 30)[0] < 0.35, "halo: {:?}", out.at(43, 30));
        // Nothing to blur: the picture comes back.
        assert_eq!(blur(&img, &vec![0.0; w * h], 8.0), img);
    }
}

#[cfg(test)]
mod scale_tests {
    use super::*;

    #[test]
    fn a_wide_blur_is_made_small_and_stays_even() {
        // A flat grey stays the same grey however wide the blur (no edge darkening, no shift).
        let (w, h) = (300, 200);
        let mut img = Rgb::new(w, h);
        img.data.fill(0.5);
        let out = blur(&img, &vec![1.0; w * h], 60.0);
        for (x, y) in [(150, 100), (2, 2), (297, 197)] {
            let v = out.at(x, y)[0];
            assert!((v - 0.5).abs() < 0.01, "({x}, {y}): {v}");
        }
        // A sharp edge is spread far (beyond the direct radius).
        let mut edge = Rgb::new(w, h);
        for y in 0..h {
            for x in 150..w {
                edge.data[(y * w + x) * 3..(y * w + x) * 3 + 3].fill(1.0);
            }
        }
        let out = blur(&edge, &vec![1.0; w * h], 60.0);
        assert!(out.at(120, 100)[0] > 0.2, "{:?}", out.at(120, 100));
    }
}
