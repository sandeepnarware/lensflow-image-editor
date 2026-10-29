//! Who a face is: SFace's description of it, for finding a person across the catalogue.
//! Faces are YuNet's ([`crate::faces`]). Each is first aligned the way SFace was trained: its
//! five landmarks are carried onto the standard 112 × 112 face (ArcFace's template) by the
//! rotation, scale and shift that fit them best ([`fit`]), and the picture is resampled
//! through it. SFace then answers 128 numbers; scaled to unit length, the cosine of two
//! faces' descriptions says how alike they are (OpenCV's same-person threshold is 0.363).
//!
//! Faces too small to tell apart (eyes closer than [`MIN_EYE_DISTANCE`] at the scanned
//! size) are left out: a crowd in the background is not people to find.

use crate::assets::{self, Asset};
use crate::faces::Face;
use crate::{AiError, Engine, Rgb};

pub const IDENTITY_MODELS: [Asset; 2] = [assets::FACES, assets::IDENTITY];

/// The long edge photos are scanned at (as for culling: faces are found at the same size).
pub const SCAN_EDGE: u32 = crate::cull::SCORE_EDGE;
/// The smallest eye distance (pixels at [`SCAN_EDGE`]) a face is described at.
pub const MIN_EYE_DISTANCE: f32 = 18.0;
/// The edge of the square face picture kept for the People list.
pub const THUMB_EDGE: usize = 96;
/// How many numbers describe a face.
pub const LENGTH: usize = 128;

const ALIGNED: usize = 112;
/// Where the right eye, left eye, nose tip and right and left mouth corners sit on the
/// aligned face (ArcFace's template, as OpenCV's `FaceRecognizerSF::alignCrop` uses it).
const TEMPLATE: [[f32; 2]; 5] = [
    [38.2946, 51.6963],
    [73.5318, 51.5014],
    [56.0252, 71.7366],
    [41.5493, 92.3655],
    [70.7299, 92.2041],
];

/// The rotation-and-scale `[a, b]` and shift `[tx, ty]` that best carry `from` onto `to`
/// (least squares, no mirroring): `x' = a·x − b·y + tx`, `y' = b·x + a·y + ty`.
pub fn fit(from: &[[f32; 2]; 5], to: &[[f32; 2]; 5]) -> [f32; 4] {
    let mean = |p: &[[f32; 2]; 5]| {
        let (sx, sy) = p.iter().fold((0.0, 0.0), |(x, y), q| (x + q[0], y + q[1]));
        [sx / 5.0, sy / 5.0]
    };
    let (mf, mt) = (mean(from), mean(to));
    let (mut dot, mut cross, mut norm) = (0.0f32, 0.0f32, 0.0f32);
    for (f, t) in from.iter().zip(to) {
        let (fx, fy) = (f[0] - mf[0], f[1] - mf[1]);
        let (tx, ty) = (t[0] - mt[0], t[1] - mt[1]);
        dot += fx * tx + fy * ty;
        cross += fx * ty - fy * tx;
        norm += fx * fx + fy * fy;
    }
    let norm = norm.max(1e-6);
    let (a, b) = (dot / norm, cross / norm);
    [
        a,
        b,
        mt[0] - (a * mf[0] - b * mf[1]),
        mt[1] - (b * mf[0] + a * mf[1]),
    ]
}

/// `img` at `(x, y)`, bilinear; black outside (as OpenCV's warp fills).
fn sample(img: &Rgb, x: f32, y: f32) -> [f32; 3] {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let at = |xi: f32, yi: f32| -> [f32; 3] {
        if xi < 0.0 || yi < 0.0 || xi >= img.w as f32 || yi >= img.h as f32 {
            [0.0; 3]
        } else {
            img.at(xi as usize, yi as usize)
        }
    };
    let (p00, p10, p01, p11) = (
        at(x0, y0),
        at(x0 + 1.0, y0),
        at(x0, y0 + 1.0),
        at(x0 + 1.0, y0 + 1.0),
    );
    [0, 1, 2].map(|c| {
        let top = p00[c] + (p10[c] - p00[c]) * fx;
        let bottom = p01[c] + (p11[c] - p01[c]) * fx;
        top + (bottom - top) * fy
    })
}

/// `face` of `img` aligned to the 112 × 112 template.
pub fn aligned(img: &Rgb, face: &Face) -> Rgb {
    let [a, b, tx, ty] = fit(&face.landmarks, &TEMPLATE);
    // Each aligned pixel comes from the inverse of the fit.
    let k = 1.0 / (a * a + b * b).max(1e-12);
    let mut out = Rgb::new(ALIGNED, ALIGNED);
    for v in 0..ALIGNED {
        for u in 0..ALIGNED {
            let (dx, dy) = (u as f32 - tx, v as f32 - ty);
            let (x, y) = ((a * dx + b * dy) * k, (a * dy - b * dx) * k);
            let i = (v * ALIGNED + u) * 3;
            out.data[i..i + 3].copy_from_slice(&sample(img, x, y));
        }
    }
    out
}

/// A square picture of `face` with some room round it (inside `img`), [`THUMB_EDGE`] wide.
pub fn thumb(img: &Rgb, face: &Face) -> Rgb {
    let side = (face.w.max(face.h) * 1.4)
        .min(img.w.min(img.h) as f32)
        .max(1.0);
    let (cx, cy) = (face.x + face.w / 2.0, face.y + face.h / 2.0);
    let x = (cx - side / 2.0).clamp(0.0, img.w as f32 - side) as usize;
    let y = (cy - side / 2.0).clamp(0.0, img.h as f32 - side) as usize;
    let side = side as usize;
    img.crop(x, y, side, side).resized(THUMB_EDGE, THUMB_EDGE)
}

/// `v` scaled to unit length (all zeros stays zeros).
pub fn unit(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 1e-12 {
        v.iter_mut().for_each(|x| *x /= n);
    }
    v
}

/// How alike two unit descriptions are: their cosine, −1..1.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// A face found and described.
#[derive(Debug, Clone)]
pub struct Found {
    /// Where it is, in the scanned picture's pixels.
    pub face: Face,
    /// [`LENGTH`] numbers, unit length.
    pub description: Vec<f32>,
    pub thumb: Rgb,
}

/// Finds and describes faces with both models loaded once.
pub struct Identifier {
    faces: ort::session::Session,
    sface: ort::session::Session,
}

impl Identifier {
    /// The models must be ready ([`Engine::prepare`] with [`IDENTITY_MODELS`]).
    pub fn new(engine: &Engine) -> Result<Identifier, AiError> {
        Ok(Identifier {
            faces: engine.session(&assets::FACES)?,
            sface: engine.session(&assets::IDENTITY)?,
        })
    }

    /// The faces of `img` large enough to tell apart, described.
    pub fn find(&mut self, img: &Rgb) -> Result<Vec<Found>, AiError> {
        let mut out = Vec::new();
        for face in crate::faces::detect_with(&mut self.faces, img)? {
            let [r, l] = [face.landmarks[0], face.landmarks[1]];
            if (r[0] - l[0]).hypot(r[1] - l[1]) < MIN_EYE_DISTANCE {
                continue;
            }
            out.push(Found {
                face,
                description: self.describe(img, &face)?,
                thumb: thumb(img, &face),
            });
        }
        Ok(out)
    }

    /// `face` of `img` described: [`LENGTH`] numbers, unit length.
    pub fn describe(&mut self, img: &Rgb, face: &Face) -> Result<Vec<f32>, AiError> {
        // RGB, 0..255, planar: SFace's graph takes away the mean and scales itself.
        let mut input = aligned(img, face).planar();
        input.iter_mut().for_each(|v| *v *= 255.0);
        let (_, values) = crate::run_one(&mut self.sface, "data", [1, 3, ALIGNED, ALIGNED], input)?;
        if values.len() != LENGTH {
            return Err(AiError::Model(format!(
                "the face model answered {} numbers, not {LENGTH}",
                values.len()
            )));
        }
        Ok(unit(values))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face_at(landmarks: [[f32; 2]; 5]) -> Face {
        Face {
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
            score: 0.9,
            landmarks,
        }
    }

    #[test]
    fn the_fit_undoes_a_turn_a_scale_and_a_shift() {
        // The template turned by 30°, doubled and moved.
        let (s, c) = (30f32.to_radians().sin(), 30f32.to_radians().cos());
        let moved =
            TEMPLATE.map(|[x, y]| [2.0 * (c * x - s * y) + 100.0, 2.0 * (s * x + c * y) + 40.0]);
        let [a, b, tx, ty] = fit(&moved, &TEMPLATE);
        for (m, t) in moved.iter().zip(&TEMPLATE) {
            let back = [a * m[0] - b * m[1] + tx, b * m[0] + a * m[1] + ty];
            assert!((back[0] - t[0]).abs() < 1e-3 && (back[1] - t[1]).abs() < 1e-3);
        }
        assert!((a.hypot(b) - 0.5).abs() < 1e-5, "the scale halves");
    }

    #[test]
    fn alignment_puts_the_landmarks_on_the_template() {
        // A picture twice the template's size with a bright dot at each (doubled) landmark.
        let (w, h) = (224, 224);
        let mut img = Rgb::new(w, h);
        let landmarks = TEMPLATE.map(|[x, y]| [x * 2.0, y * 2.0]);
        for [lx, ly] in landmarks {
            for y in 0..h {
                for x in 0..w {
                    if (x as f32 - lx).hypot(y as f32 - ly) < 3.0 {
                        img.data[(y * w + x) * 3..(y * w + x) * 3 + 3].fill(1.0);
                    }
                }
            }
        }
        let out = aligned(&img, &face_at(landmarks));
        assert_eq!((out.w, out.h), (ALIGNED, ALIGNED));
        for [tx, ty] in TEMPLATE {
            let v = out.at(tx.round() as usize, ty.round() as usize)[0];
            assert!(v > 0.9, "no dot at ({tx}, {ty}): {v}");
        }
        assert_eq!(out.at(2, 2), [0.0; 3]);
    }

    #[test]
    fn a_thumb_is_square_and_stays_inside_the_picture() {
        let img = Rgb::new(300, 200);
        let mut f = face_at(TEMPLATE);
        (f.x, f.y, f.w, f.h) = (260.0, -10.0, 60.0, 80.0);
        let t = thumb(&img, &f);
        assert_eq!((t.w, t.h), (THUMB_EDGE, THUMB_EDGE));
        // A face larger than the picture still makes a thumb.
        (f.x, f.y, f.w, f.h) = (0.0, 0.0, 500.0, 500.0);
        assert_eq!(thumb(&img, &f).w, THUMB_EDGE);
    }

    #[test]
    fn descriptions_are_compared_by_cosine() {
        let a = unit(vec![3.0, 4.0]);
        assert!((a[0] - 0.6).abs() < 1e-6);
        assert!((cosine(&a, &a) - 1.0).abs() < 1e-6);
        assert!(cosine(&a, &unit(vec![-4.0, 3.0])).abs() < 1e-6);
        assert_eq!(unit(vec![0.0, 0.0]), vec![0.0, 0.0]);
    }
}
