//! How far things are: Depth Anything V2 Small (Apache-2.0) answers relative inverse depth
//! (larger is nearer) for a photo seen at about 518 pixels on its shorter side, each side a
//! multiple of 14 (the network's patch). [`depth`] scales the answer to 0 (farthest) .. 1
//! (nearest) and back to the size asked.

use crate::assets::{self, Asset};
use crate::{AiError, Engine, Gray, Rgb};

pub const DEPTH_MODELS: [Asset; 1] = [assets::DEPTH];

const SHORT: usize = 518;
const PATCH: usize = 14;
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// The network's input size for a `w × h` picture: the shorter side about 518, both sides
/// multiples of 14.
pub fn input_size(w: usize, h: usize) -> (usize, usize) {
    let k = SHORT as f64 / w.min(h).max(1) as f64;
    let round = |v: usize| ((((v as f64) * k) / PATCH as f64).round() as usize).max(1) * PATCH;
    (round(w), round(h))
}

/// `values` scaled so the smallest is 0 and the largest 1 (all 0 when flat).
pub fn normalised(values: &[f32]) -> Vec<f32> {
    let (lo, hi) = values
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
    let span = hi - lo;
    if !span.is_finite() || span <= 1e-9 {
        return vec![0.0; values.len()];
    }
    values.iter().map(|v| (v - lo) / span).collect()
}

/// How near each pixel of `img` is, 0 (farthest) .. 1 (nearest), as a `w × h` map.
pub fn depth(engine: &Engine, img: &Rgb, w: usize, h: usize) -> Result<Gray, AiError> {
    let (iw, ih) = input_size(img.w, img.h);
    let mut input = img.resized(iw, ih).planar();
    let n = iw * ih;
    for c in 0..3 {
        for v in &mut input[c * n..(c + 1) * n] {
            *v = (*v - MEAN[c]) / STD[c];
        }
    }
    let mut session = engine.session(&assets::DEPTH)?;
    let (shape, values) = crate::run_one(&mut session, "pixel_values", [1, 3, ih, iw], input)?;
    let (oh, ow) = match shape.as_slice() {
        [_, oh, ow] => (*oh, *ow),
        [_, _, oh, ow] => (*oh, *ow),
        s => return Err(AiError::Model(format!("unexpected depth {s:?}"))),
    };
    if oh * ow != values.len() {
        return Err(AiError::Model(format!("unexpected depth {shape:?}")));
    }
    Ok(Gray {
        w: ow,
        h: oh,
        data: normalised(&values),
    }
    .resized(w, h))
}

/// Where near and far divide in `depth` (0..1): Otsu's threshold over its histogram (the
/// split that best separates two groups of distances).
pub fn split(depth: &[f32]) -> f32 {
    const BINS: usize = 64;
    let mut hist = [0usize; BINS];
    for &d in depth {
        hist[((d.clamp(0.0, 1.0) * (BINS - 1) as f32).round()) as usize] += 1;
    }
    let total = depth.len().max(1) as f64;
    let sum: f64 = hist
        .iter()
        .enumerate()
        .map(|(i, &n)| i as f64 * n as f64)
        .sum();
    // Across an empty gap between the groups every split scores the same: the middle of the
    // best-scoring run is taken, not its first bin.
    let (mut w0, mut s0, mut best) = (0.0f64, 0.0f64, -1.0f64);
    let (mut first, mut last) = (BINS / 2, BINS / 2);
    for (i, &n) in hist.iter().enumerate() {
        w0 += n as f64;
        s0 += i as f64 * n as f64;
        let w1 = total - w0;
        if w0 == 0.0 || w1 == 0.0 {
            continue;
        }
        let between = w0 * w1 * (s0 / w0 - (sum - s0) / w1).powi(2);
        if between > best * (1.0 + 1e-9) {
            best = between;
            (first, last) = (i, i);
        } else if between >= best * (1.0 - 1e-9) && last + 1 == i {
            last = i;
        }
    }
    ((first + last) as f32 / 2.0 + 0.5) / (BINS - 1) as f32
}

/// A `w × h` mask of what is near (`near`) or far in `img`, softened a little round the split.
pub fn mask(engine: &Engine, img: &Rgb, near: bool, w: usize, h: usize) -> Result<Gray, AiError> {
    let d = depth(engine, img, w, h)?;
    Ok(Gray {
        w,
        h,
        data: side(&d.data, near),
    })
}

/// Each depth's share of the near (or far) side of [`split`], with a soft edge 0.08 wide.
pub fn side(depth: &[f32], near: bool) -> Vec<f32> {
    let t = split(depth);
    depth
        .iter()
        .map(|&d| {
            let k = ((d - (t - 0.04)) / 0.08).clamp(0.0, 1.0);
            let k = k * k * (3.0 - 2.0 * k);
            if near { k } else { 1.0 - k }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_input_is_about_518_short_and_in_patches() {
        assert_eq!(input_size(512, 341), (784, 518));
        let (w, h) = input_size(4000, 6000);
        assert_eq!((w % 14, h % 14), (0, 0));
        assert_eq!(w, 518);
    }

    #[test]
    fn near_and_far_divide_between_the_two_groups() {
        // A far background (around 0.2) and a near subject (around 0.8).
        let mut d: Vec<f32> = (0..600).map(|i| 0.15 + (i % 10) as f32 * 0.01).collect();
        d.extend((0..400).map(|i| 0.75 + (i % 10) as f32 * 0.01));
        let t = split(&d);
        assert!(t > 0.3 && t < 0.7, "{t}");
        let near = side(&d, true);
        assert!(near[0] < 0.01 && near[700] > 0.99);
        let far = side(&d, false);
        assert!(far[0] > 0.99 && far[700] < 0.01);
    }

    #[test]
    fn values_are_spread_to_0_1() {
        assert_eq!(normalised(&[2.0, 4.0, 3.0]), vec![0.0, 1.0, 0.5]);
        assert_eq!(normalised(&[1.0, 1.0]), vec![0.0, 0.0]);
    }
}
