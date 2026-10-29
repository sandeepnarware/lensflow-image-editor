//! "Select object": a click on any object masks it, with MobileSAM (Apache-2.0; Segment
//! Anything's prompt decoder). The image encoder describes the photo once (its longest side
//! scaled to 1024; the graph normalises and pads itself); the decoder then answers masks for
//! the clicked point. The multi-mask decoder offers several readings of one click (a shirt,
//! the person wearing it, the group); the one it rates best is kept.

use crate::assets::{self, Asset};
use crate::{AiError, Engine, Gray, Rgb};

pub const OBJECT_MODELS: [Asset; 2] = [assets::SAM_ENCODER, assets::SAM_DECODER];

const EDGE: usize = 1024;

/// The size `w × h` is scaled to so its longest side is the encoder's 1024.
pub fn encoder_size(w: usize, h: usize) -> (usize, usize) {
    let k = EDGE as f64 / w.max(h).max(1) as f64;
    (
        ((w as f64 * k).round() as usize).clamp(1, EDGE),
        ((h as f64 * k).round() as usize).clamp(1, EDGE),
    )
}

/// The clicked point (`u, v` in 0..1 of the photo) in the encoder's frame, followed by SAM's
/// padding point (no box): coordinates and labels.
pub fn prompt(u: f32, v: f32, w: usize, h: usize) -> ([f32; 4], [f32; 2]) {
    let (rw, rh) = encoder_size(w, h);
    (
        [
            u.clamp(0.0, 1.0) * rw as f32,
            v.clamp(0.0, 1.0) * rh as f32,
            0.0,
            0.0,
        ],
        [1.0, -1.0],
    )
}

/// The best-rated of `n` masks laid out one after another, each `len` long.
pub fn best<'a>(masks: &'a [f32], scores: &[f32], len: usize) -> Option<&'a [f32]> {
    let i = scores
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(i, _)| i)?;
    masks.get(i * len..(i + 1) * len)
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// The object at `(u, v)` (0..1 of `img`) as a `w × h` mask (0..1); `w × h` should have
/// `img`'s shape.
pub fn object_mask(
    engine: &Engine,
    img: &Rgb,
    (u, v): (f32, f32),
    w: usize,
    h: usize,
) -> Result<Gray, AiError> {
    let model = |e: ort::Error| AiError::Model(e.to_string());
    let (rw, rh) = encoder_size(img.w, img.h);
    let resized = img.resized(rw, rh);
    // RGB, 0..255, height × width × 3 with no batch: the graph does the rest.
    let pixels: Vec<f32> = resized.data.iter().map(|v| v * 255.0).collect();
    let mut encoder = engine.session(&assets::SAM_ENCODER)?;
    let tensor = ort::value::Tensor::from_array(([rh, rw, 3], pixels)).map_err(model)?;
    let outputs = encoder
        .run(ort::inputs!["input_image" => tensor])
        .map_err(model)?;
    let (shape, embeddings) = outputs["image_embeddings"]
        .try_extract_tensor::<f32>()
        .map_err(model)?;
    let shape: Vec<usize> = shape.iter().map(|&d| d.max(0) as usize).collect();
    let embeddings = embeddings.to_vec();
    drop(outputs);

    let (coords, labels) = prompt(u, v, img.w, img.h);
    let mut decoder = engine.session(&assets::SAM_DECODER)?;
    let t = |shape: Vec<usize>, data: Vec<f32>| {
        ort::value::Tensor::from_array((shape, data)).map_err(model)
    };
    let outputs = decoder
        .run(ort::inputs![
            "image_embeddings" => t(shape, embeddings)?,
            "point_coords" => t(vec![1, 2, 2], coords.to_vec())?,
            "point_labels" => t(vec![1, 2], labels.to_vec())?,
            "mask_input" => t(vec![1, 1, 256, 256], vec![0.0; 256 * 256])?,
            "has_mask_input" => t(vec![1], vec![0.0])?,
            "orig_im_size" => t(vec![2], vec![h as f32, w as f32])?,
        ])
        .map_err(model)?;
    let (mshape, masks) = outputs["masks"]
        .try_extract_tensor::<f32>()
        .map_err(model)?;
    let (_, scores) = outputs["iou_predictions"]
        .try_extract_tensor::<f32>()
        .map_err(model)?;
    let mshape: Vec<usize> = mshape.iter().map(|&d| d.max(0) as usize).collect();
    if mshape.len() != 4 || mshape[2] != h || mshape[3] != w {
        return Err(AiError::Model(format!("unexpected masks {mshape:?}")));
    }
    let logits = best(masks, scores, w * h)
        .ok_or_else(|| AiError::Model("the object model answered no mask".to_owned()))?;
    Ok(Gray {
        w,
        h,
        // The logits' sigmoid: firm inside, a narrow soft edge.
        data: logits.iter().map(|&l| sigmoid(l * 2.0)).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_longest_side_becomes_1024() {
        assert_eq!(encoder_size(4000, 3000), (1024, 768));
        assert_eq!(encoder_size(500, 1000), (512, 1024));
        assert_eq!(encoder_size(0, 0), (1, 1));
    }

    #[test]
    fn a_click_is_a_point_in_the_encoders_frame_then_padding() {
        let (coords, labels) = prompt(0.5, 0.25, 4000, 3000);
        assert_eq!(coords, [512.0, 192.0, 0.0, 0.0]);
        assert_eq!(labels, [1.0, -1.0]);
    }

    #[test]
    fn the_best_rated_mask_is_kept() {
        let masks = [0.0, 0.0, 1.0, 1.0, 2.0, 2.0];
        assert_eq!(best(&masks, &[0.1, 0.9, 0.5], 2), Some(&[1.0, 1.0][..]));
        assert_eq!(best(&masks, &[], 2), None);
    }
}
