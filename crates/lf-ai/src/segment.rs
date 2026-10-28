//! Masks from the picture itself: the main subject (IS-Net, general use), the sky (SkySeg, a
//! U²-Net) and people and their parts (MediaPipe's selfie multiclass segmenter: hair, face
//! skin, body skin, clothes). Each works on a small square copy of the photo and gives a
//! 0..1 mask the size asked for. The subject and sky models' first output is already a
//! sigmoid, so it is used as it is (a photo with no sky gives an empty mask, not a stretched
//! one); the people model answers logits per class, made probabilities by a softmax.

use crate::assets::{self, Asset};
use crate::{AiError, Engine, Gray, Rgb, run_one};

pub const SUBJECT_MODELS: [Asset; 1] = [assets::SUBJECT];
pub const SKY_MODELS: [Asset; 1] = [assets::SKY];
pub const PEOPLE_MODELS: [Asset; 1] = [assets::PEOPLE];

const SUBJECT_EDGE: usize = 1024;
const SKY_EDGE: usize = 320;
const PEOPLE_EDGE: usize = 256;
/// The people model's classes, in its output's order.
const PEOPLE_CLASSES: usize = 6;
/// ImageNet's mean and spread (the sky model's training).
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// What a mask is of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Subject,
    Sky,
    /// Everyone in the photo, whole (everything the people model does not call background).
    People,
    Hair,
    FaceSkin,
    BodySkin,
    Clothes,
}

impl Target {
    /// Every target, in the order the mask tool offers them.
    pub const ALL: [Target; 7] = [
        Target::Subject,
        Target::Sky,
        Target::People,
        Target::FaceSkin,
        Target::BodySkin,
        Target::Hair,
        Target::Clothes,
    ];

    pub fn models(self) -> &'static [Asset] {
        match self {
            Target::Subject => &SUBJECT_MODELS,
            Target::Sky => &SKY_MODELS,
            _ => &PEOPLE_MODELS,
        }
    }

    /// The name a mask of it is saved under (`Shape::Image::what`).
    pub fn key(self) -> &'static str {
        match self {
            Target::Subject => "subject",
            Target::Sky => "sky",
            Target::People => "people",
            Target::Hair => "hair",
            Target::FaceSkin => "face-skin",
            Target::BodySkin => "body-skin",
            Target::Clothes => "clothes",
        }
    }

    /// The mask tool's button.
    pub fn button(self) -> &'static str {
        match self {
            Target::Subject => "Select subject",
            Target::Sky => "Select sky",
            Target::People => "People",
            Target::Hair => "Hair",
            Target::FaceSkin => "Face skin",
            Target::BodySkin => "Body skin",
            Target::Clothes => "Clothes",
        }
    }

    /// What it is, in a sentence ("No hair was found.").
    pub fn noun(self) -> &'static str {
        match self {
            Target::Subject => "subject",
            Target::Sky => "sky",
            Target::People => "person",
            Target::Hair => "hair",
            Target::FaceSkin => "face",
            Target::BodySkin => "skin",
            Target::Clothes => "clothing",
        }
    }

    /// The people model's classes that make it up (its output channels).
    fn classes(self) -> &'static [usize] {
        match self {
            Target::Hair => &[1],
            Target::BodySkin => &[2],
            Target::FaceSkin => &[3],
            // Accessories (glasses, hats, bags) go with the clothes.
            Target::Clothes => &[4, 5],
            Target::People => &[1, 2, 3, 4, 5],
            Target::Subject | Target::Sky => &[],
        }
    }

    fn edge(self) -> usize {
        match self {
            Target::Subject => SUBJECT_EDGE,
            Target::Sky => SKY_EDGE,
            _ => PEOPLE_EDGE,
        }
    }
}

/// The input tensor for `target` from `img`: square, normalised as the model was trained.
/// Planar (NCHW) for the subject and sky; interleaved (NHWC) for the people model.
pub fn input(target: Target, img: &Rgb) -> (usize, Vec<f32>) {
    let edge = target.edge();
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
        // RGB 0..1, stretched to the square, as it is.
        _ => return (edge, square.data),
    }
    (edge, square.planar())
}

/// The probability of `classes` at each pixel of the people model's logits (NHWC, six
/// classes per pixel), by a softmax over the six.
pub fn people_probability(logits: &[f32], classes: &[usize]) -> Vec<f32> {
    logits
        .as_chunks::<PEOPLE_CLASSES>()
        .0
        .iter()
        .map(|l| {
            let top = l.iter().copied().fold(f32::MIN, f32::max);
            let e: Vec<f32> = l.iter().map(|v| (v - top).exp()).collect();
            let sum: f32 = e.iter().sum();
            classes.iter().map(|&c| e[c]).sum::<f32>() / sum.max(1e-12)
        })
        .collect()
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
        _ => (assets::PEOPLE, "input_29"),
    };
    let mut session = engine.session(&model)?;
    let (edge, data) = input(target, img);
    let out = if matches!(target, Target::Subject | Target::Sky) {
        let (shape, values) = run_one(&mut session, name, [1, 3, edge, edge], data)?;
        if shape.len() != 4 || shape[2] * shape[3] != values.len() {
            return Err(AiError::Model(format!("unexpected output {shape:?}")));
        }
        Gray {
            w: shape[3],
            h: shape[2],
            data: values.iter().map(|v| v.clamp(0.0, 1.0)).collect(),
        }
    } else {
        let (shape, values) = run_one(&mut session, name, [1, edge, edge, 3], data)?;
        if shape != [1, edge, edge, PEOPLE_CLASSES] || values.len() != edge * edge * PEOPLE_CLASSES
        {
            return Err(AiError::Model(format!("unexpected output {shape:?}")));
        }
        Gray {
            w: edge,
            h: edge,
            data: people_probability(&values, target.classes()),
        }
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
        let (edge, data) = input(Target::Hair, &img);
        assert_eq!((edge, data.len()), (256, 256 * 256 * 3));
        assert!(data.iter().all(|v| (v - 0.5).abs() < 1e-5));
    }

    #[test]
    fn people_classes_come_from_a_softmax() {
        // One pixel that is surely hair, one surely background.
        let logits = [
            0.0, 10.0, 0.0, 0.0, 0.0, 0.0, //
            10.0, 0.0, 0.0, 0.0, 0.0, 0.0,
        ];
        let hair = people_probability(&logits, Target::Hair.classes());
        assert!(hair[0] > 0.99 && hair[1] < 0.01, "{hair:?}");
        let people = people_probability(&logits, Target::People.classes());
        assert!(people[0] > 0.99 && people[1] < 0.01, "{people:?}");
        // Even odds: each of the six is a sixth.
        let even = people_probability(&[1.0; 6], Target::Clothes.classes());
        assert!((even[0] - 2.0 / 6.0).abs() < 1e-6);
    }

    #[test]
    fn targets_have_distinct_keys_and_models() {
        let keys: std::collections::HashSet<_> = Target::ALL.iter().map(|t| t.key()).collect();
        assert_eq!(keys.len(), Target::ALL.len());
        assert_eq!(Target::Sky.models(), &SKY_MODELS);
        assert_eq!(Target::FaceSkin.models(), &PEOPLE_MODELS);
    }
}
