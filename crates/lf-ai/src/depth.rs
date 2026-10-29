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
    fn values_are_spread_to_0_1() {
        assert_eq!(normalised(&[2.0, 4.0, 3.0]), vec![0.0, 1.0, 0.5]);
        assert_eq!(normalised(&[1.0, 1.0]), vec![0.0, 0.0]);
    }
}
