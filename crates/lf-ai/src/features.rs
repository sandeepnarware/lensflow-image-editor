//! Masks of the eyes, irises, lips and teeth, from MediaPipe's face landmarker (Face Mesh V2,
//! Apache-2.0: 478 points on a face, irises included). Each face YuNet finds
//! ([`crate::faces`]) is cut out as MediaPipe does it: a square 1.5 times the face's box,
//! turned so the eyes are level, at 256 × 256. The model answers its points in that square,
//! which are carried back onto the photo. The regions are MediaPipe's own outlines
//! (`face_mesh_connections.py`) filled in: the eye openings, the irises (a circle through
//! each iris's ring), the lips (the outer outline less the mouth's opening), and the teeth
//! (the mouth's opening, where it is light and colourless as teeth are).

use crate::assets::{self, Asset};
use crate::faces::Face;
use crate::{AiError, Engine, Gray, Rgb};

pub const FEATURE_MODELS: [Asset; 2] = [assets::FACES, assets::FACE_MESH];

/// A part of a face.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Eyes,
    Irises,
    Lips,
    Teeth,
}

const CROP: usize = 256;
const POINTS: usize = 478;
/// The face's box is grown this much for the cut-out (MediaPipe's own scale).
const ROI_SCALE: f32 = 1.5;

/// The subject's left eye's opening, then the right's (MediaPipe's FACEMESH_LEFT_EYE and
/// FACEMESH_RIGHT_EYE, in order round the outline).
const LEFT_EYE: [usize; 16] = [
    263, 249, 390, 373, 374, 380, 381, 382, 362, 398, 384, 385, 386, 387, 388, 466,
];
const RIGHT_EYE: [usize; 16] = [
    33, 7, 163, 144, 145, 153, 154, 155, 133, 173, 157, 158, 159, 160, 161, 246,
];
/// The lips' outer outline and the mouth's opening (from FACEMESH_LIPS).
const OUTER_LIPS: [usize; 20] = [
    61, 146, 91, 181, 84, 17, 314, 405, 321, 375, 291, 409, 270, 269, 267, 0, 37, 39, 40, 185,
];
const INNER_LIPS: [usize; 20] = [
    78, 95, 88, 178, 87, 14, 317, 402, 318, 324, 308, 415, 310, 311, 312, 13, 82, 81, 80, 191,
];
/// Each iris: its centre, then its ring (the 478-point model's last ten points).
const IRISES: [[usize; 5]; 2] = [[468, 469, 470, 471, 472], [473, 474, 475, 476, 477]];

/// The square cut out for `face`: its centre, side and turn (radians; the eyes level).
pub fn roi(face: &Face) -> ([f32; 2], f32, f32) {
    let [r, l] = [face.landmarks[0], face.landmarks[1]];
    let angle = (l[1] - r[1]).atan2(l[0] - r[0]);
    let centre = [face.x + face.w / 2.0, face.y + face.h / 2.0];
    (centre, face.w.max(face.h) * ROI_SCALE, angle)
}

/// A point of the 256 × 256 cut-out in the photo's pixels.
pub fn to_photo(p: [f32; 2], (centre, side, angle): ([f32; 2], f32, f32)) -> [f32; 2] {
    let k = side / CROP as f32;
    let (dx, dy) = (
        (p[0] - CROP as f32 / 2.0) * k,
        (p[1] - CROP as f32 / 2.0) * k,
    );
    let (s, c) = angle.sin_cos();
    [centre[0] + c * dx - s * dy, centre[1] + s * dx + c * dy]
}

/// The cut-out of `img` for `roi`, NHWC RGB 0..1 (black outside the photo).
fn cut_out(img: &Rgb, roi: ([f32; 2], f32, f32)) -> Vec<f32> {
    let mut out = vec![0.0; CROP * CROP * 3];
    for v in 0..CROP {
        for u in 0..CROP {
            let [x, y] = to_photo([u as f32 + 0.5, v as f32 + 0.5], roi);
            let (xi, yi) = (x.floor(), y.floor());
            if xi < 0.0 || yi < 0.0 || xi >= img.w as f32 || yi >= img.h as f32 {
                continue;
            }
            let i = (v * CROP + u) * 3;
            out[i..i + 3].copy_from_slice(&img.at(xi as usize, yi as usize));
        }
    }
    out
}

/// The 478 points of the face in `roi` of `img` (photo pixels), or `None` when the model
/// says it is not a face.
fn landmarks(
    session: &mut ort::session::Session,
    img: &Rgb,
    roi: ([f32; 2], f32, f32),
) -> Result<Option<Vec<[f32; 2]>>, AiError> {
    let model = |e: ort::Error| AiError::Model(e.to_string());
    let tensor = ort::value::Tensor::from_array(([1usize, CROP, CROP, 3], cut_out(img, roi)))
        .map_err(model)?;
    let outputs = session
        .run(ort::inputs!["input_12" => tensor])
        .map_err(model)?;
    let (_, points) = outputs["Identity"]
        .try_extract_tensor::<f32>()
        .map_err(model)?;
    let (_, presence) = outputs["Identity_1"]
        .try_extract_tensor::<f32>()
        .map_err(model)?;
    if points.len() < POINTS * 3 {
        return Err(AiError::Model(format!(
            "the face landmark model answered {} numbers",
            points.len()
        )));
    }
    if presence.first().is_none_or(|&l| l < 0.0) {
        return Ok(None);
    }
    Ok(Some(
        points
            .chunks(3)
            .take(POINTS)
            .map(|p| to_photo([p[0], p[1]], roi))
            .collect(),
    ))
}

/// How much of the `w × h` mask pixel at `(x, y)` lies inside `poly` (4 × 4 samples).
fn coverage(poly: &[[f32; 2]], x: usize, y: usize) -> f32 {
    let mut n = 0;
    for sy in 0..4 {
        for sx in 0..4 {
            let p = [
                x as f32 + (sx as f32 + 0.5) / 4.0,
                y as f32 + (sy as f32 + 0.5) / 4.0,
            ];
            n += usize::from(inside(poly, p));
        }
    }
    n as f32 / 16.0
}

/// Even-odd point-in-polygon.
fn inside(poly: &[[f32; 2]], p: [f32; 2]) -> bool {
    let mut odd = false;
    let mut j = poly.len().wrapping_sub(1);
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            odd = !odd;
        }
        j = i;
    }
    odd
}

/// Adds `poly`'s coverage (times `weight` at each pixel) into `mask`.
fn fill(mask: &mut Gray, poly: &[[f32; 2]], weight: impl Fn(usize, usize) -> f32) {
    if poly.len() < 3 {
        return;
    }
    let (x0, x1) = poly
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p[0]), b.max(p[0])));
    let (y0, y1) = poly
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p[1]), b.max(p[1])));
    let xs = (x0.floor().max(0.0) as usize)..(x1.ceil().max(0.0) as usize).min(mask.w);
    for y in (y0.floor().max(0.0) as usize)..(y1.ceil().max(0.0) as usize).min(mask.h) {
        for x in xs.clone() {
            let c = coverage(poly, x, y);
            if c > 0.0 {
                let m = &mut mask.data[y * mask.w + x];
                *m = m.max(c * weight(x, y));
            }
        }
    }
}

/// Takes `inner`'s coverage out of `mask` (the lips less the mouth's opening).
fn cut(mask: &mut Gray, inner: &[[f32; 2]]) {
    let mut hole = Gray {
        w: mask.w,
        h: mask.h,
        data: vec![0.0; mask.data.len()],
    };
    fill(&mut hole, inner, |_, _| 1.0);
    for (m, h) in mask.data.iter_mut().zip(&hole.data) {
        *m = (*m - h).max(0.0);
    }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// How tooth-like a colour is: light and nearly colourless.
fn tooth_like([r, g, b]: [f32; 3]) -> f32 {
    let luma = 0.299 * r + 0.587 * g + 0.114 * b;
    let chroma = r.max(g).max(b) - r.min(g).min(b);
    smoothstep(0.25, 0.45, luma) * (1.0 - smoothstep(0.18, 0.35, chroma))
}

/// `part` of every face's `points` (photo pixels of `img`) drawn into a `w × h` mask.
pub fn draw(part: Part, faces: &[Vec<[f32; 2]>], img: &Rgb, w: usize, h: usize) -> Gray {
    let mut mask = Gray {
        w,
        h,
        data: vec![0.0; w * h],
    };
    let (kx, ky) = (
        w as f32 / img.w.max(1) as f32,
        h as f32 / img.h.max(1) as f32,
    );
    let poly = |pts: &[[f32; 2]], idx: &[usize]| -> Vec<[f32; 2]> {
        idx.iter()
            .filter_map(|&i| pts.get(i))
            .map(|p| [p[0] * kx, p[1] * ky])
            .collect()
    };
    for pts in faces {
        match part {
            Part::Eyes => {
                fill(&mut mask, &poly(pts, &LEFT_EYE), |_, _| 1.0);
                fill(&mut mask, &poly(pts, &RIGHT_EYE), |_, _| 1.0);
            }
            Part::Irises => {
                for iris in IRISES {
                    let p = poly(pts, &iris);
                    if p.len() < 5 {
                        continue;
                    }
                    let c = p[0];
                    let r = p[1..]
                        .iter()
                        .map(|q| (q[0] - c[0]).hypot(q[1] - c[1]))
                        .sum::<f32>()
                        / 4.0;
                    let circle: Vec<[f32; 2]> = (0..24)
                        .map(|k| {
                            let a = k as f32 / 24.0 * std::f32::consts::TAU;
                            [c[0] + r * a.cos(), c[1] + r * a.sin()]
                        })
                        .collect();
                    // Only the part the eyelids leave open.
                    let mut disc = Gray {
                        w,
                        h,
                        data: vec![0.0; w * h],
                    };
                    fill(&mut disc, &circle, |_, _| 1.0);
                    let mut open = Gray {
                        w,
                        h,
                        data: vec![0.0; w * h],
                    };
                    fill(&mut open, &poly(pts, &LEFT_EYE), |_, _| 1.0);
                    fill(&mut open, &poly(pts, &RIGHT_EYE), |_, _| 1.0);
                    for ((m, d), o) in mask.data.iter_mut().zip(&disc.data).zip(&open.data) {
                        *m = m.max(d.min(*o));
                    }
                }
            }
            Part::Lips => {
                let mut lips = Gray {
                    w,
                    h,
                    data: vec![0.0; w * h],
                };
                fill(&mut lips, &poly(pts, &OUTER_LIPS), |_, _| 1.0);
                cut(&mut lips, &poly(pts, &INNER_LIPS));
                for (m, l) in mask.data.iter_mut().zip(&lips.data) {
                    *m = m.max(*l);
                }
            }
            Part::Teeth => {
                let (sx, sy) = (img.w as f32 / w as f32, img.h as f32 / h as f32);
                fill(&mut mask, &poly(pts, &INNER_LIPS), |x, y| {
                    let px = ((x as f32 + 0.5) * sx) as usize;
                    let py = ((y as f32 + 0.5) * sy) as usize;
                    tooth_like(img.at(px.min(img.w - 1), py.min(img.h - 1)))
                });
            }
        }
    }
    mask
}

/// A `w × h` mask of `part` on every face in `img`.
pub fn mask(engine: &Engine, part: Part, img: &Rgb, w: usize, h: usize) -> Result<Gray, AiError> {
    let faces = crate::faces::detect(engine, img)?;
    let mut session = engine.session(&assets::FACE_MESH)?;
    let mut found = Vec::new();
    for face in &faces {
        if let Some(points) = landmarks(&mut session, img, roi(face))? {
            found.push(points);
        }
    }
    Ok(draw(part, &found, img, w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cut_out_is_centred_turned_and_sized_on_the_face() {
        let face = Face {
            x: 100.0,
            y: 100.0,
            w: 100.0,
            h: 100.0,
            score: 0.9,
            // The eyes level: no turn.
            landmarks: [
                [130.0, 140.0],
                [170.0, 140.0],
                [150.0, 160.0],
                [135.0, 180.0],
                [165.0, 180.0],
            ],
        };
        let r = roi(&face);
        assert_eq!(r.0, [150.0, 150.0]);
        assert_eq!(r.1, 150.0);
        assert_eq!(r.2, 0.0);
        assert_eq!(to_photo([128.0, 128.0], r), [150.0, 150.0]);
        assert_eq!(to_photo([0.0, 0.0], r), [75.0, 75.0]);
        // Eyes on a slope of 45°: the cut-out's right is down-right in the photo.
        let mut tilted = face;
        tilted.landmarks[1] = [170.0, 180.0];
        let p = to_photo([256.0, 128.0], roi(&tilted));
        assert!((p[0] - p[1]).abs() < 1e-3 && p[0] > 150.0, "{p:?}");
    }

    #[test]
    fn polygons_fill_with_soft_edges_and_holes_cut() {
        let mut m = Gray {
            w: 20,
            h: 20,
            data: vec![0.0; 400],
        };
        let square = [[2.0, 2.0], [12.0, 2.0], [12.0, 12.0], [2.0, 12.0]];
        fill(&mut m, &square, |_, _| 1.0);
        assert_eq!(m.data[5 * 20 + 5], 1.0);
        assert_eq!(m.data[15 * 20 + 15], 0.0);
        let half = [[0.0, 0.0], [16.5, 0.0], [16.5, 1.0], [0.0, 1.0]];
        fill(&mut m, &half, |_, _| 1.0);
        assert_eq!(m.data[16], 0.5, "a pixel half inside is half covered");
        cut(&mut m, &[[4.0, 4.0], [8.0, 4.0], [8.0, 8.0], [4.0, 8.0]]);
        assert_eq!(m.data[5 * 20 + 5], 0.0);
        assert_eq!(m.data[10 * 20 + 10], 1.0);
    }

    #[test]
    fn teeth_are_light_and_colourless() {
        assert!(tooth_like([0.9, 0.88, 0.85]) > 0.9);
        assert!(tooth_like([0.7, 0.2, 0.25]) < 0.1, "lips and gums");
        assert!(
            tooth_like([0.08, 0.06, 0.06]) < 0.1,
            "the mouth's dark inside"
        );
    }

    #[test]
    fn regions_are_drawn_from_the_points_at_the_masks_size() {
        // A made-up face: all 478 points at the origin, but the mouth's.
        let mut pts = vec![[0.0f32, 0.0]; POINTS];
        let (cx, cy) = (100.0f32, 100.0f32);
        for (k, &i) in OUTER_LIPS.iter().enumerate() {
            let a = k as f32 / 20.0 * std::f32::consts::TAU;
            pts[i] = [cx + 40.0 * a.cos(), cy + 20.0 * a.sin()];
        }
        for (k, &i) in INNER_LIPS.iter().enumerate() {
            let a = k as f32 / 20.0 * std::f32::consts::TAU;
            pts[i] = [cx + 25.0 * a.cos(), cy + 8.0 * a.sin()];
        }
        let mut img = Rgb::new(200, 200);
        img.data.iter_mut().for_each(|v| *v = 0.9);
        let lips = draw(Part::Lips, &[pts.clone()], &img, 100, 100);
        // Half size: the mouth's middle is open, the lip above it is lips.
        assert_eq!(lips.data[50 * 100 + 50], 0.0);
        assert_eq!(lips.data[42 * 100 + 50], 1.0);
        let teeth = draw(Part::Teeth, &[pts], &img, 100, 100);
        assert!(teeth.data[50 * 100 + 50] > 0.9, "a light, grey opening");
        assert_eq!(teeth.data[42 * 100 + 50], 0.0);
    }
}
