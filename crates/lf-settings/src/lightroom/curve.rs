//! Lightroom's tone curves in our terms: its parametric curve folded into the point curve,
//! and curves sampled down to the points we keep.
//!
//! We hold a curve as at most [`MAX_CURVE_POINTS`] points; Lightroom holds a point curve plus
//! a parametric curve laid under it. When the parametric curve changes anything, the two are
//! composed and sampled at evenly spaced points; otherwise the point curve is kept as it is.

use crate::CurvePoint;
use crate::curve::curve_table;

/// Most points a curve keeps (`normalise` drops the rest).
pub const MAX_CURVE_POINTS: usize = 16;
/// How far a parametric region at ±100 raises or lowers the curve.
pub const PARAMETRIC_REACH: f64 = 0.25;

/// The point curve `points` (0..1, sorted) with the parametric curve `params` laid under it:
/// the seven parametric keys in the order shadows, darks, lights, highlights, shadow split,
/// midtone split, highlight split (`&[]` for none).
pub fn shaped(points: Vec<CurvePoint>, params: &[Option<f64>]) -> Vec<CurvePoint> {
    let regions = Parametric::new(params);
    if !regions.is_flat() {
        sample(&points, MAX_CURVE_POINTS, |x| regions.apply(x))
    } else if points.len() > MAX_CURVE_POINTS {
        sample(&points, MAX_CURVE_POINTS, |x| x)
    } else {
        points
    }
}

/// Lightroom's parametric curve: four regions (shadows, darks, lights, highlights) split at
/// three points. Each region raises or lowers the curve by up to [`PARAMETRIC_REACH`] at
/// ±100, most at its own middle and fading (smoothstep) to nothing at the middle of the
/// regions either side of it (the end regions fade to 0 and 1), so neighbours overlap and the
/// ends of the curve stay put.
struct Parametric {
    amounts: [f64; 4],
    /// 0, the three splits, 1.
    bounds: [f64; 5],
}

impl Parametric {
    /// From the seven parametric keys, in the order shadows, darks, lights, highlights,
    /// shadow split, midtone split, highlight split (Lightroom's defaults 25, 50, 75).
    fn new(params: &[Option<f64>]) -> Parametric {
        let p = |i: usize, d: f64| params.get(i).copied().flatten().unwrap_or(d);
        let mut splits = [p(4, 25.0), p(5, 50.0), p(6, 75.0)].map(|v| (v / 100.0).clamp(0.0, 1.0));
        splits.sort_by(f64::total_cmp);
        Parametric {
            amounts: std::array::from_fn(|i| p(i, 0.0).clamp(-100.0, 100.0) / 100.0),
            bounds: [0.0, splits[0], splits[1], splits[2], 1.0],
        }
    }

    /// No region raises or lowers anything (the splits alone change nothing).
    fn is_flat(&self) -> bool {
        self.amounts.iter().all(|a| *a == 0.0)
    }

    fn apply(&self, x: f64) -> f64 {
        let mid = |i: usize| (self.bounds[i] + self.bounds[i + 1]) / 2.0;
        let mut y = x;
        for (i, amount) in self.amounts.iter().enumerate() {
            if *amount == 0.0 {
                continue;
            }
            let c = mid(i);
            let lo = if i == 0 { 0.0 } else { mid(i - 1) };
            let hi = if i == 3 { 1.0 } else { mid(i + 1) };
            let w = if x <= c {
                smoothstep(lo, c, x)
            } else {
                1.0 - smoothstep(c, hi, x)
            };
            y += amount * PARAMETRIC_REACH * w;
        }
        y.clamp(0.0, 1.0)
    }
}

fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    if e1 <= e0 {
        return if x < e0 { 0.0 } else { 1.0 };
    }
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The point curve through `points` after `before`, at `n` evenly spaced inputs.
fn sample(points: &[CurvePoint], n: usize, before: impl Fn(f64) -> f64) -> Vec<CurvePoint> {
    const SIZE: usize = 1024;
    let table = curve_table(points, SIZE);
    let at = |x: f64| {
        let f = x.clamp(0.0, 1.0) * (SIZE - 1) as f64;
        let i = (f.floor() as usize).min(SIZE - 2);
        let t = f - i as f64;
        f64::from(table[i]) * (1.0 - t) + f64::from(table[i + 1]) * t
    };
    (0..n)
        .map(|i| {
            let x = i as f64 / (n - 1) as f64;
            [x, at(before(x))]
        })
        .collect()
}
