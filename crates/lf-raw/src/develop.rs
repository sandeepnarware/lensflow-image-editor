//! Develop sensor data into a picture the render pipeline can take.
//!
//! [`develop`] runs, in order:
//!
//! 1. demosaic ([`crate::demosaic::run`]): camera RGB, 0..1 = the sensor's range;
//! 2. white balance: multiply by the as-shot `wb` (green = 1);
//! 3. colour: [`color::cam_to_srgb`] (identity when the file has no matrix);
//! 4. highlights: `Clip` clamps each channel at 1.0; `Reconstruct` blends pixels whose
//!    largest channel `m` exceeds 1.0 toward neutral `(Y, Y, Y)` (Rec. 709 luminance)
//!    by `t = clamp((m - 1) / m * 2, 0, 1)`, keeping `Y`, which may stay above 1.0;
//! 5. base curve ([`curve::apply`]);
//! 6. downscale, when the long edge exceeds `max_edge`: an integer box filter of
//!    factor `ceil(long_edge / max_edge)`, averaged in linear light (this step runs
//!    before gamma, which the brief allows and which is cheaper and exact);
//! 7. gamma encode `pow(max(x, 0), 1 / 2.2)`, no clamp;
//! 8. orientation (EXIF 1..8) and conversion to RGBA f16 (alpha 1).
//!
//! Steps 2 to 5 are one pass over the demosaiced buffer, in place.

use crate::demosaic::{self, Quality};
use crate::{Orientation, RawError, RawFrame, color, curve};
use half::f16;
use image::RgbaImage;
use lf_pipeline::FloatFrame;
use lf_settings::{DevelopSettings, Highlights};
use rayon::prelude::*;

/// Gamma used to encode the developed linear values (and by the 8-bit path's files).
pub const GAMMA: f32 = 2.2;

/// Rec. 709 / sRGB luminance weights.
const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// A developed picture.
#[derive(Debug, Clone)]
pub struct Developed {
    /// RGBA f16, row 0 = top, gamma encoded, unclamped; oriented upright.
    pub frame: FloatFrame,
    /// Size before any reduction (after orientation, so it has the frame's aspect).
    pub full_width: u32,
    pub full_height: u32,
    /// True when the frame was box-downscaled to fit `max_edge`.
    pub reduced: bool,
    /// True when the EXIF orientation was not 1 and the buffer was turned or flipped.
    pub orientation_applied: bool,
}

/// Develop `raw` (see the module docs for the steps).
pub fn develop(
    raw: &RawFrame,
    quality: Quality,
    settings: &DevelopSettings,
    max_edge: u32,
) -> Result<Developed, RawError> {
    let rgb = demosaic::run(raw, quality);
    let (w, h) = (rgb.width, rgb.height);
    if w == 0 || h == 0 {
        return Err(RawError::Empty);
    }
    let mut data = rgb.data;

    let wb = [raw.wb[0], raw.wb[1], raw.wb[2]];
    let m = color::cam_to_srgb(raw.xyz_to_cam, raw.matrix_white);
    let (profile, highlights) = (settings.profile, settings.highlights);
    data.par_chunks_mut(3 * w as usize).for_each(|row| {
        for p in row.as_chunks_mut::<3>().0 {
            let balanced = [p[0] * wb[0], p[1] * wb[1], p[2] * wb[2]];
            let mut c = color::apply(&m, balanced);
            match highlights {
                Highlights::Clip => c = c.map(|v| v.min(1.0)),
                Highlights::Reconstruct => reconstruct(&mut c),
            }
            for (out, v) in p.iter_mut().zip(c) {
                *out = curve::apply(profile, v);
            }
        }
    });

    let long = w.max(h);
    let max_edge = max_edge.max(1);
    let (data, w, h, reduced) = if long > max_edge {
        let factor = long.div_ceil(max_edge);
        let (d, nw, nh) = box_downscale(&data, w, h, factor);
        (d, nw, nh, true)
    } else {
        (data, w, h, false)
    };

    let inv_gamma = 1.0 / GAMMA;
    let o = raw.orientation;
    let (full_width, full_height) = oriented_size(o, rgb.width, rgb.height);
    let frame = orient_to_f16(w, h, o, |i| {
        let s = &data[i * 3..i * 3 + 3];
        [
            s[0].max(0.0).powf(inv_gamma),
            s[1].max(0.0).powf(inv_gamma),
            s[2].max(0.0).powf(inv_gamma),
            1.0,
        ]
    });
    Ok(Developed {
        frame,
        full_width,
        full_height,
        reduced,
        orientation_applied: o.exif() != 1,
    })
}

/// An 8-bit picture (JPEG, PNG, TIFF, WebP) as a developed frame: oriented, each byte
/// `v` becomes the f16 nearest `v / 255` (no curve, no matrix), so it renders as an
/// 8-bit upload does.
pub fn develop_rgb8(img: &RgbaImage, orientation: Orientation) -> Developed {
    let (w, h) = img.dimensions();
    let bytes = img.as_raw();
    let frame = orient_to_f16(w, h, orientation, |i| {
        let s = &bytes[i * 4..i * 4 + 4];
        s.try_into()
            .map(|p: [u8; 4]| p.map(|v| v as f32 / 255.0))
            .unwrap_or([0.0; 4])
    });
    let (full_width, full_height) = (frame.width, frame.height);
    Developed {
        frame,
        full_width,
        full_height,
        reduced: false,
        orientation_applied: orientation.exif() != 1,
    }
}

/// Pull an over-range colour toward neutral, keeping its luminance.
#[inline]
fn reconstruct(c: &mut [f32; 3]) {
    let m = c[0].max(c[1]).max(c[2]);
    if m <= 1.0 {
        return;
    }
    let t = ((m - 1.0) / m.max(1e-6) * 2.0).clamp(0.0, 1.0);
    let y = LUMA[0] * c[0] + LUMA[1] * c[1] + LUMA[2] * c[2];
    for v in c.iter_mut() {
        *v += t * (y - *v);
    }
}

/// Average `factor × factor` blocks of interleaved RGB (partial blocks at the right
/// and bottom edges average the pixels they have).
fn box_downscale(data: &[f32], w: u32, h: u32, factor: u32) -> (Vec<f32>, u32, u32) {
    let (nw, nh) = (w.div_ceil(factor), h.div_ceil(factor));
    let (w, h, f) = (w as usize, h as usize, factor as usize);
    let mut out = vec![0.0f32; nw as usize * nh as usize * 3];
    out.par_chunks_mut(nw as usize * 3)
        .enumerate()
        .for_each(|(oy, row)| {
            let (y0, y1) = (oy * f, ((oy + 1) * f).min(h));
            for (ox, px) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                let (x0, x1) = (ox * f, ((ox + 1) * f).min(w));
                let mut sum = [0.0f64; 3];
                for y in y0..y1 {
                    for x in x0..x1 {
                        let i = (y * w + x) * 3;
                        for c in 0..3 {
                            sum[c] += data[i + c] as f64;
                        }
                    }
                }
                let n = ((y1 - y0) * (x1 - x0)) as f64;
                for c in 0..3 {
                    px[c] = (sum[c] / n) as f32;
                }
            }
        });
    (out, nw, nh)
}

/// Size after applying the orientation to a `w × h` buffer.
fn oriented_size(o: Orientation, w: u32, h: u32) -> (u32, u32) {
    if o.exif() >= 5 { (h, w) } else { (w, h) }
}

/// Source pixel (in a `w × h` buffer) shown at output pixel `(ox, oy)` under EXIF
/// orientation `o`: 2 mirror, 3 rotate 180, 4 flip, 5 transpose, 6 rotate 90° clockwise,
/// 7 transverse, 8 rotate 90° anticlockwise.
fn source_of(o: Orientation, w: usize, h: usize, ox: usize, oy: usize) -> (usize, usize) {
    match o.exif() {
        2 => (w - 1 - ox, oy),
        3 => (w - 1 - ox, h - 1 - oy),
        4 => (ox, h - 1 - oy),
        5 => (oy, ox),
        6 => (oy, h - 1 - ox),
        7 => (w - 1 - oy, h - 1 - ox),
        8 => (w - 1 - oy, ox),
        _ => (ox, oy),
    }
}

/// Build an oriented RGBA f16 frame from a `w × h` source; `pixel(i)` gives the
/// RGBA value of source pixel index `i = y * w + x`.
fn orient_to_f16(
    w: u32,
    h: u32,
    o: Orientation,
    pixel: impl Fn(usize) -> [f32; 4] + Sync,
) -> FloatFrame {
    let (ow, oh) = oriented_size(o, w, h);
    let mut data = vec![f16::ZERO; ow as usize * oh as usize * 4];
    if ow > 0 {
        data.par_chunks_mut(ow as usize * 4)
            .enumerate()
            .for_each(|(oy, row)| {
                for (ox, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                    let (sx, sy) = source_of(o, w as usize, h as usize, ox, oy);
                    let v = pixel(sy * w as usize + sx);
                    for c in 0..4 {
                        px[c] = f16::from_f32(v[c]);
                    }
                }
            });
    }
    FloatFrame {
        width: ow,
        height: oh,
        data,
    }
}
