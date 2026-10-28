//! Masks from the picture itself: the main subject (IS-Net, general use) and the sky
//! (SkySeg, a U²-Net). Both work on a small square copy of the photo and give a 0..1 mask
//! the size asked for; their first output is already a sigmoid, so it is used as it is (a
//! photo with no sky gives an empty mask, not a stretched one).

use crate::assets::{self, Asset};
use crate::{AiError, Engine, Gray, Rgb, run_one};

pub const SUBJECT_MODELS: [Asset; 1] = [assets::SUBJECT];
pub const SKY_MODELS: [Asset; 1] = [assets::SKY];

const SUBJECT_EDGE: usize = 1024;
const SKY_EDGE: usize = 320;
/// ImageNet's mean and spread (the sky model's training).
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// What a mask is of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Subject,
    Sky,
}

impl Target {
    pub fn models(self) -> &'static [Asset] {
        match self {
            Target::Subject => &SUBJECT_MODELS,
            Target::Sky => &SKY_MODELS,
        }
    }
}

/// The input tensor for `target` from `img`: square, normalised as the model was trained.
pub fn input(target: Target, img: &Rgb) -> (usize, Vec<f32>) {
    let edge = match target {
        Target::Subject => SUBJECT_EDGE,
        Target::Sky => SKY_EDGE,
    };
    let mut square = img.resized(edge, edge);
    match target {
        // rembg's preparation: scaled so the brightest value is 1, then centred on 0.5.
        Target::Subject => {
            let top = square.data.iter().copied().fold(1e-6f32, f32::max);
            for v in &mut square.data {
                *v = *v / top - 0.5;
            }
        }
        Target::Sky => {
            for (i, v) in square.data.iter_mut().enumerate() {
                let c = i % 3;
                *v = (*v - MEAN[c]) / STD[c];
            }
        }
    }
    (edge, square.planar())
}

/// A mask of `target` in `img`, sized `w × h`.
pub fn mask(
    engine: &Engine,
    target: Target,
    img: &Rgb,
    w: usize,
    h: usize,
) -> Result<Gray, AiError> {
    let (model, name) = match target {
        Target::Subject => (assets::SUBJECT, "input_image"),
        Target::Sky => (assets::SKY, "input.1"),
    };
    let mut session = engine.session(&model)?;
    let (edge, data) = input(target, img);
    let (shape, values) = run_one(&mut session, name, [1, 3, edge, edge], data)?;
    if shape.len() != 4 || shape[2] * shape[3] != values.len() {
        return Err(AiError::Model(format!("unexpected output {shape:?}")));
    }
    let out = Gray {
        w: shape[3],
        h: shape[2],
        data: values.iter().map(|v| v.clamp(0.0, 1.0)).collect(),
    };
    Ok(out.resized(w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inputs_are_square_and_normalised() {
        let img = Rgb {
            w: 4,
            h: 2,
            data: vec![0.5; 24],
        };
        let (edge, data) = input(Target::Sky, &img);
        assert_eq!((edge, data.len()), (320, 3 * 320 * 320));
        assert!((data[0] - (0.5 - 0.485) / 0.229).abs() < 1e-5);
        let (edge, data) = input(Target::Subject, &img);
        assert_eq!(edge, 1024);
        // Brightest scaled to 1, then − 0.5.
        assert!((data[0] - 0.5).abs() < 1e-5);
    }
}
