//! Faces (YuNet) and portrait retouching. YuNet takes a 640 × 640 BGR picture with values
//! 0..255 and answers, at strides 8, 16 and 32, a class and an objectness score, a box and
//! five landmarks per grid cell ([`decode`], as OpenCV's `FaceDetectorYN` reads them).
//!
//! Retouching is plain image processing on the faces found: skin (the colours near the
//! face's own, inside an ellipse round the face, eyes and mouth left out) is smoothed with
//! an edge-aware blur, keeping a little of its fine texture so it does not look plastic.

use crate::assets::{self, Asset};
use crate::{AiError, Engine, Rgb};

pub const FACE_MODELS: [Asset; 1] = [assets::FACES];

const EDGE: usize = 640;
const STRIDES: [usize; 3] = [8, 16, 32];
/// The lowest score kept, and how much two faces may overlap before one is dropped.
const MIN_SCORE: f32 = 0.7;
const NMS_IOU: f32 = 0.3;

/// A face found: its box and landmarks (eyes, nose tip, mouth corners) in the picture's
/// pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Face {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub score: f32,
    /// Right eye, left eye, nose, right and left mouth corner.
    pub landmarks: [[f32; 2]; 5],
}

/// The model's outputs at one stride: per grid cell, `cls` and `obj` (1 each), `bbox` (4),
/// `kps` (10).
pub struct Level<'a> {
    pub stride: usize,
    pub cls: &'a [f32],
    pub obj: &'a [f32],
    pub bbox: &'a [f32],
    pub kps: &'a [f32],
}

/// The faces in the model's answer (in the 640 × 640 input's pixels), best first, overlaps
/// removed.
pub fn decode(levels: &[Level]) -> Vec<Face> {
    let mut found = Vec::new();
    for l in levels {
        let cols = EDGE / l.stride;
        let s = l.stride as f32;
        for i in 0..l.cls.len().min(l.obj.len()) {
            let score = (l.cls[i].clamp(0.0, 1.0) * l.obj[i].clamp(0.0, 1.0)).sqrt();
            if score < MIN_SCORE || l.bbox.len() < (i + 1) * 4 || l.kps.len() < (i + 1) * 10 {
                continue;
            }
            let (r, c) = ((i / cols) as f32, (i % cols) as f32);
            let b = &l.bbox[i * 4..i * 4 + 4];
            let (cx, cy) = ((c + b[0]) * s, (r + b[1]) * s);
            let (w, h) = (b[2].exp() * s, b[3].exp() * s);
            let k = &l.kps[i * 10..i * 10 + 10];
            let mut landmarks = [[0.0; 2]; 5];
            for (n, p) in landmarks.iter_mut().enumerate() {
                *p = [(k[n * 2] + c) * s, (k[n * 2 + 1] + r) * s];
            }
            found.push(Face {
                x: cx - w / 2.0,
                y: cy - h / 2.0,
                w,
                h,
                score,
                landmarks,
            });
        }
    }
    found.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Face> = Vec::new();
    for f in found {
        if kept.iter().all(|k| iou(k, &f) <= NMS_IOU) {
            kept.push(f);
        }
    }
    kept
}

fn iou(a: &Face, b: &Face) -> f32 {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.w).min(b.x + b.w);
    let y1 = (a.y + a.h).min(b.y + b.h);
    let inter = (x1 - x0).max(0.0) * (y1 - y0).max(0.0);
    inter / (a.w * a.h + b.w * b.h - inter).max(1e-6)
}

/// The faces in `img` (any size: it is fitted into the model's square, then mapped back).
pub fn detect(engine: &Engine, img: &Rgb) -> Result<Vec<Face>, AiError> {
    detect_with(&mut engine.session(&assets::FACES)?, img)
}

/// [`detect`] with a YuNet session already made (scoring many photos makes it once).
pub fn detect_with(session: &mut ort::session::Session, img: &Rgb) -> Result<Vec<Face>, AiError> {
    let scale = EDGE as f32 / img.w.max(img.h) as f32;
    let (fw, fh) = (
        ((img.w as f32 * scale).round() as usize).clamp(1, EDGE),
        ((img.h as f32 * scale).round() as usize).clamp(1, EDGE),
    );
    let fitted = img.resized(fw, fh);
    // BGR, 0..255, in the top-left of a black square.
    let n = EDGE * EDGE;
    let mut input = vec![0.0f32; 3 * n];
    for y in 0..fh {
        for x in 0..fw {
            let [r, g, b] = fitted.at(x, y);
            let i = y * EDGE + x;
            input[i] = b * 255.0;
            input[n + i] = g * 255.0;
            input[2 * n + i] = r * 255.0;
        }
    }
    let model = |e: ort::Error| AiError::Model(e.to_string());
    let tensor = ort::value::Tensor::from_array(([1usize, 3, EDGE, EDGE], input)).map_err(model)?;
    let outputs = session
        .run(ort::inputs!["input" => tensor])
        .map_err(model)?;
    let get = |name: String| -> Result<Vec<f32>, AiError> {
        let (_, v) = outputs[name.as_str()]
            .try_extract_tensor::<f32>()
            .map_err(model)?;
        Ok(v.to_vec())
    };
    let mut held = Vec::new();
    for s in STRIDES {
        held.push((
            s,
            get(format!("cls_{s}"))?,
            get(format!("obj_{s}"))?,
            get(format!("bbox_{s}"))?,
            get(format!("kps_{s}"))?,
        ));
    }
    let levels: Vec<Level> = held
        .iter()
        .map(|(s, cls, obj, bbox, kps)| Level {
            stride: *s,
            cls,
            obj,
            bbox,
            kps,
        })
        .collect();
    let back = 1.0 / scale;
    Ok(decode(&levels)
        .into_iter()
        .map(|f| Face {
            x: f.x * back,
            y: f.y * back,
            w: f.w * back,
            h: f.h * back,
            score: f.score,
            landmarks: f.landmarks.map(|[x, y]| [x * back, y * back]),
        })
        .collect())
}

/// Chroma of an RGB colour (BT.601 Cb, Cr), the skin test's space.
fn chroma(c: [f32; 3]) -> [f32; 2] {
    let y = 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2];
    [(c[2] - y) * 0.564, (c[0] - y) * 0.713]
}

/// Where the skin of `faces` is in `img` (0..1), eyes and mouth left out.
pub fn skin_mask(img: &Rgb, faces: &[Face]) -> Vec<f32> {
    let mut mask = vec![0.0f32; img.w * img.h];
    for f in faces {
        // The face's own skin colour: the median chroma of the cheeks and forehead (the box's
        // middle third, a little up).
        let (x0, y0) = (
            (f.x + f.w * 0.3).max(0.0) as usize,
            (f.y + f.h * 0.25).max(0.0) as usize,
        );
        let (x1, y1) = (
            ((f.x + f.w * 0.7) as usize).min(img.w),
            ((f.y + f.h * 0.6) as usize).min(img.h),
        );
        let mut cbs = Vec::new();
        let mut crs = Vec::new();
        for y in y0..y1 {
            for x in x0..x1 {
                let [cb, cr] = chroma(img.at(x, y));
                cbs.push(cb);
                crs.push(cr);
            }
        }
        if cbs.is_empty() {
            continue;
        }
        let median = |v: &mut Vec<f32>| {
            v.sort_by(f32::total_cmp);
            v[v.len() / 2]
        };
        let skin = [median(&mut cbs), median(&mut crs)];
        // An ellipse a little larger than the box (the neck and ears too).
        let (cx, cy) = (f.x + f.w / 2.0, f.y + f.h * 0.55);
        let (rx, ry) = (f.w * 0.75, f.h * 0.9);
        let keep_out: Vec<([f32; 2], f32)> = vec![
            (f.landmarks[0], f.w * 0.13),
            (f.landmarks[1], f.w * 0.13),
            (
                [
                    (f.landmarks[3][0] + f.landmarks[4][0]) / 2.0,
                    (f.landmarks[3][1] + f.landmarks[4][1]) / 2.0,
                ],
                f.w * 0.2,
            ),
        ];
        let bx0 = (cx - rx).max(0.0) as usize;
        let bx1 = ((cx + rx) as usize).min(img.w);
        let by0 = (cy - ry).max(0.0) as usize;
        let by1 = ((cy + ry) as usize).min(img.h);
        for y in by0..by1 {
            for x in bx0..bx1 {
                let (dx, dy) = ((x as f32 - cx) / rx, (y as f32 - cy) / ry);
                let d = (dx * dx + dy * dy).sqrt();
                if d >= 1.0 {
                    continue;
                }
                let edge = 1.0 - smoothstep(0.7, 1.0, d);
                let [cb, cr] = chroma(img.at(x, y));
                let off = ((cb - skin[0]).powi(2) + (cr - skin[1]).powi(2)).sqrt();
                let like = 1.0 - smoothstep(0.03, 0.07, off);
                let mut away = 1.0f32;
                for (p, r) in &keep_out {
                    let dd = ((x as f32 - p[0]).powi(2) + (y as f32 - p[1]).powi(2)).sqrt();
                    away = away.min(smoothstep(r * 0.7, *r, dd));
                }
                let m = &mut mask[y * img.w + x];
                *m = m.max(edge * like * away);
            }
        }
    }
    mask
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A box blur of radius `r` (two passes through a running sum: quick at any radius).
fn box_blur(img: &Rgb, r: usize) -> Rgb {
    let pass = |src: &Rgb, horizontal: bool| -> Rgb {
        let mut out = Rgb::new(src.w, src.h);
        let (len, lines) = if horizontal {
            (src.w, src.h)
        } else {
            (src.h, src.w)
        };
        let at = |line: usize, i: usize| {
            if horizontal {
                line * src.w + i
            } else {
                i * src.w + line
            }
        };
        for line in 0..lines {
            for c in 0..3 {
                let mut sum = 0.0f32;
                let n = (2 * r + 1) as f32;
                for k in 0..=2 * r {
                    let i = k.saturating_sub(r).min(len - 1);
                    sum += src.data[at(line, i) * 3 + c];
                }
                for i in 0..len {
                    out.data[at(line, i) * 3 + c] = sum / n;
                    let leave = i.saturating_sub(r).min(len - 1);
                    let enter = (i + r + 1).min(len - 1);
                    sum += src.data[at(line, enter) * 3 + c] - src.data[at(line, leave) * 3 + c];
                }
            }
        }
        out
    };
    pass(&pass(img, true), false)
}

/// `img` with the skin of `faces` smoothed by `amount` (0..1).
pub fn smooth_skin(img: &Rgb, faces: &[Face], amount: f32) -> Rgb {
    let amount = amount.clamp(0.0, 1.0);
    if faces.is_empty() || amount == 0.0 {
        return img.clone();
    }
    let mask = skin_mask(img, faces);
    // The blur scales with the largest face, so a small face is not wiped out.
    let face = faces.iter().map(|f| f.w).fold(0.0, f32::max);
    let r = ((face * 0.025) as usize).clamp(2, 40);
    let fine = ((face * 0.004) as usize).clamp(1, 6);
    let smooth = box_blur(&box_blur(img, r), r / 2 + 1);
    let texture = box_blur(img, fine);
    let mut out = img.clone();
    for (i, &painted) in mask.iter().enumerate() {
        let m = painted * amount;
        if m <= 0.0 {
            continue;
        }
        for c in 0..3 {
            let k = i * 3 + c;
            let v = img.data[k];
            let s = smooth.data[k];
            // Strong edges (a lash line, a lip) stay: the blur only takes small differences.
            let guard = (-((v - s) / 0.08).powi(2)).exp();
            let detail = v - texture.data[k];
            let target = s + detail * 0.35;
            out.data[k] = (v + (target - v) * m * guard).clamp(0.0, 1.0);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_confident_cell_decodes_to_a_box_and_weak_ones_are_dropped() {
        let cells = (EDGE / 32) * (EDGE / 32);
        let mut cls = vec![0.0; cells];
        let mut obj = vec![0.0; cells];
        let mut bbox = vec![0.0; cells * 4];
        let kps = vec![0.5; cells * 10];
        // Cell (row 2, col 3): centred in it, 2 strides (64 px) square... e^ln2 = 2.
        let i = 2 * 20 + 3;
        cls[i] = 0.9;
        obj[i] = 0.9;
        bbox[i * 4..i * 4 + 4].copy_from_slice(&[0.5, 0.5, 2f32.ln(), 2f32.ln()]);
        // A neighbour overlapping it with a lower score goes.
        cls[i + 1] = 0.8;
        obj[i + 1] = 0.8;
        bbox[(i + 1) * 4..(i + 1) * 4 + 4].copy_from_slice(&[-0.5, 0.5, 2f32.ln(), 2f32.ln()]);
        // A weak one elsewhere is not a face.
        cls[100] = 0.3;
        obj[100] = 0.3;
        let faces = decode(&[Level {
            stride: 32,
            cls: &cls,
            obj: &obj,
            bbox: &bbox,
            kps: &kps,
        }]);
        assert_eq!(faces.len(), 1);
        let f = faces[0];
        assert!((f.x - (3.5 * 32.0 - 32.0)).abs() < 1e-3 && (f.w - 64.0).abs() < 1e-3);
        assert!((f.score - 0.9).abs() < 1e-5);
        assert_eq!(f.landmarks[0], [3.5 * 32.0, 2.5 * 32.0]);
    }

    #[test]
    fn skin_is_smoothed_and_the_rest_left_alone() {
        // A skin-toned face area with speckle, on a blue background.
        let (w, h) = (200, 200);
        let mut img = Rgb::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let inside = (x as f32 - 100.0).hypot(y as f32 - 110.0) < 60.0;
                let speck = if (x * 7 + y * 13) % 5 == 0 { 0.04 } else { 0.0 };
                let c = if inside {
                    [0.85 + speck, 0.65 + speck, 0.55 + speck]
                } else {
                    [0.1, 0.2, 0.6]
                };
                img.data[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&c);
            }
        }
        let face = Face {
            x: 50.0,
            y: 50.0,
            w: 100.0,
            h: 120.0,
            score: 0.95,
            landmarks: [
                [80.0, 90.0],
                [120.0, 90.0],
                [100.0, 115.0],
                [85.0, 140.0],
                [115.0, 140.0],
            ],
        };
        let out = smooth_skin(&img, &[face], 1.0);
        let spread = |p: &Rgb| {
            let vals: Vec<f32> = (90..110).map(|x| p.at(x, 105)[0]).collect();
            vals.iter().copied().fold(0.0, f32::max) - vals.iter().copied().fold(1.0, f32::min)
        };
        assert!(
            spread(&out) < spread(&img) * 0.7,
            "{} vs {}",
            spread(&out),
            spread(&img)
        );
        assert_eq!(out.at(5, 5), img.at(5, 5), "the background moved");
        // Under an eye stays as it was.
        assert_eq!(out.at(80, 90), img.at(80, 90));
    }
}
