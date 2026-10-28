//! Upright's lines: the straight edges in the photo's small sample ([`Sample`]), found with a
//! Hough transform that keeps near-vertical and near-level lines only, each cut to its
//! longest unbroken stretch. [`lf_settings::upright::fit`] then fits the corrections.

use super::auto::Sample;
use lf_settings::upright::Segment;

/// How far from vertical or level a line may lean and still count (degrees).
const MAX_LEAN: i64 = 30;
/// The most lines kept.
const MAX_LINES: usize = 40;

/// The lines in `sample`, in photo fractions.
pub fn segments(sample: &Sample) -> Vec<Segment> {
    let (w, h) = (sample.width as usize, sample.height as usize);
    if w < 16 || h < 16 {
        return Vec::new();
    }
    // Luma (square-rooted: the sample may be linear), then Sobel gradients.
    let luma: Vec<f32> = sample
        .rgb
        .iter()
        .map(|c| {
            (0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2])
                .max(0.0)
                .sqrt()
        })
        .collect();
    let at = |x: usize, y: usize| luma[y * w + x];
    let mut edges = Vec::new();
    let mut strongest = 0.0f32;
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let gx = at(x + 1, y - 1) + 2.0 * at(x + 1, y) + at(x + 1, y + 1)
                - at(x - 1, y - 1)
                - 2.0 * at(x - 1, y)
                - at(x - 1, y + 1);
            let gy = at(x - 1, y + 1) + 2.0 * at(x, y + 1) + at(x + 1, y + 1)
                - at(x - 1, y - 1)
                - 2.0 * at(x, y - 1)
                - at(x + 1, y - 1);
            let m = gx.hypot(gy);
            strongest = strongest.max(m);
            edges.push((x, y, m, gy.atan2(gx)));
        }
    }
    let floor = strongest * 0.2;
    edges.retain(|e| e.2 > floor);
    // Hough: the line's normal angle θ (degrees, 0..180) and its distance ρ from the centre.
    let diag = ((w * w + h * h) as f64).sqrt();
    let rhos = (2.0 * diag) as usize + 1;
    let wanted = |t: i64| t <= MAX_LEAN || t >= 180 - MAX_LEAN || (t - 90).abs() <= MAX_LEAN;
    let mut votes = vec![0u32; 180 * rhos];
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    let rho = |x: usize, y: usize, t: i64| {
        let th = (t as f64).to_radians();
        (x as f64 - cx) * th.cos() + (y as f64 - cy) * th.sin()
    };
    for &(x, y, _, dir) in &edges {
        // The gradient is across the edge: only angles near it.
        let g = (f64::from(dir).to_degrees().rem_euclid(180.0)).round() as i64;
        for t in g - 4..=g + 4 {
            let t = t.rem_euclid(180);
            if !wanted(t) {
                continue;
            }
            let r = (rho(x, y, t) + diag).round() as usize;
            votes[t as usize * rhos + r.min(rhos - 1)] += 1;
        }
    }
    // Peaks, strongest first, none within 3° and 4 px of a stronger one.
    let mut order: Vec<usize> = (0..votes.len()).filter(|&i| votes[i] > 0).collect();
    order.sort_by(|a, b| votes[*b].cmp(&votes[*a]));
    let min_votes = (w.min(h) / 10).max(8) as u32;
    let mut peaks: Vec<(i64, f64)> = Vec::new();
    for i in order {
        if votes[i] < min_votes || peaks.len() >= MAX_LINES * 2 {
            break;
        }
        let (t, r) = ((i / rhos) as i64, (i % rhos) as f64 - diag);
        let near = peaks.iter().any(|&(pt, pr)| {
            let dt = (pt - t).abs().min(180 - (pt - t).abs());
            dt <= 3 && (pr - r).abs() <= 4.0
        });
        if !near {
            peaks.push((t, r));
        }
    }
    // Each peak's longest unbroken run of edge pixels along it.
    // A pixel belongs to one line at most: the strongest takes it, so a slightly turned copy
    // of a line (a weaker peak beside it) is left with too little to count.
    let mut used = vec![false; edges.len()];
    let mut out = Vec::new();
    for (t, r) in peaks {
        let th = (t as f64).to_radians();
        let (nx, ny) = (th.cos(), th.sin());
        let pos = |x: usize, y: usize| -(x as f64 - cx) * ny + (y as f64 - cy) * nx;
        let mut along: Vec<f64> = edges
            .iter()
            .zip(&used)
            .filter(|(e, used)| !**used && (rho(e.0, e.1, t) - r).abs() <= 1.0)
            .map(|(e, _)| pos(e.0, e.1))
            .collect();
        along.sort_by(f64::total_cmp);
        let (mut best, mut start) = ((0.0, 0.0), 0usize);
        for i in 1..=along.len() {
            if i == along.len() || along[i] - along[i - 1] > 3.0 {
                let (a, b) = (along[start], along[i - 1]);
                if b - a > best.1 - best.0 {
                    best = (a, b);
                }
                start = i;
            }
        }
        let length = best.1 - best.0;
        if length < w.min(h) as f64 * 0.12 {
            continue;
        }
        for (e, u) in edges.iter().zip(used.iter_mut()) {
            let s = pos(e.0, e.1);
            if (rho(e.0, e.1, t) - r).abs() <= 2.0 && s >= best.0 - 2.0 && s <= best.1 + 2.0 {
                *u = true;
            }
        }
        let point = |s: f64| {
            let x = cx + r * nx - s * ny;
            let y = cy + r * ny + s * nx;
            (x / w as f64, y / h as f64)
        };
        out.push(Segment {
            a: point(best.0),
            b: point(best.1),
            weight: length,
        });
        if out.len() >= MAX_LINES {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaning_columns_are_found_as_near_vertical_lines() {
        // Dark columns on a light wall, leaning in towards the top.
        let (w, h) = (240u32, 180u32);
        let mut rgb = vec![[0.8f32; 3]; (w * h) as usize];
        for y in 0..h {
            let lean = (h - y) as f32 * 0.08;
            for (base, dir) in [(50.0, 1.0), (190.0, -1.0)] {
                let centre: f32 = base + dir * lean;
                for x in 0..w {
                    if (x as f32 - centre).abs() < 4.0 {
                        rgb[(y * w + x) as usize] = [0.1; 3];
                    }
                }
            }
        }
        let sample = Sample {
            width: w,
            height: h,
            rgb,
        };
        let segs = segments(&sample);
        assert!(segs.len() >= 2, "{segs:?}");
        for s in &segs {
            let (dx, dy) = ((s.b.0 - s.a.0) * 240.0, (s.b.1 - s.a.1) * 180.0);
            assert!(dy.abs() > dx.abs(), "near vertical: {s:?}");
            assert!(dy.abs() > 90.0, "long: {s:?}");
        }
        // The fit straightens them: positive vertical (they lean in at the top).
        let fit = lf_settings::upright::fit(
            &segs,
            240.0 / 180.0,
            &lf_settings::lens::Lens::default(),
            lf_settings::upright::Mode::Vertical,
        )
        .unwrap();
        assert!(fit.vertical > 5.0, "{fit:?}");
    }
}
