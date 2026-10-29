//! Develop sensor data into a picture the render pipeline can take.
//!
//! [`develop`] runs, in order:
//!
//! 1. demosaic ([`crate::demosaic::run`]): camera RGB, 0..1 = the sensor's range
//!    (`Quality::Preview` bins instead: half size for Bayer, a third for X-Trans);
//! 2. white balance: multiply by the as-shot `wb` (green = 1);
//! 3. colour: [`color::cam_to_srgb`] (identity when the file has no matrix), or the chosen
//!    camera profile ([`crate::dcp`], fixed for the shot's white balance), with the
//!    highlight handling around it:
//!    - `Clip` (dcraw style): the white-balanced camera RGB is clamped at 1.0 *before*
//!      the matrix, so a clipped sensor area (camera white after WB is (1, 1, 1),
//!      which the matrix maps to sRGB white) stays neutral instead of turning magenta;
//!      the matrix output is clamped at 1.0 too;
//!    - `Reconstruct`: driven by sensor clipping, not by the converted colour. With
//!      `m` the largest *camera* channel before white balance, a pixel is blended
//!      toward neutral `(Y, Y, Y)` (Rec. 709 luminance of the converted colour) by
//!      `t = clamp((m - 0.98) / 0.02, 0, 1)`, keeping `Y`, which may stay above 1.0.
//!      Unclipped colours (`m <= 0.98`) are untouched, even when the matrix takes
//!      them above 1.0;
//! 4. base curve ([`curve::apply`]), unless the camera profile brought its own tone curve;
//! 5. downscale, when the long edge exceeds `max_edge`: an integer box filter of
//!    factor `ceil(long_edge / max_edge)`, averaged in linear light (this step runs
//!    before gamma, which the brief allows and which is cheaper and exact);
//! 6. gamma encode `pow(max(x, 0), 1 / 2.2)`, no clamp;
//! 7. orientation (EXIF 1..8) and conversion to RGBA f16 (alpha 1).
//!
//! Steps 2 to 4 are one pass over the demosaiced buffer, in place.

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

/// Part of the frame cache key: bump whenever develop output changes for the same input.
pub const DEVELOP_ALGO_VERSION: u32 = 2;

impl Developed {
    /// An 8-bit picture (already upright) as a developed frame: the same gamma-encoded values,
    /// as halves; `full` is the photo's size before `img` was made smaller.
    pub fn from_rgba8(img: &RgbaImage, full: (u32, u32)) -> Developed {
        let data = img
            .as_raw()
            .iter()
            .map(|&v| f16::from_f32(f32::from(v) / 255.0))
            .collect();
        Developed {
            frame: FloatFrame {
                width: img.width(),
                height: img.height(),
                data,
            },
            full_width: full.0,
            full_height: full.1,
            reduced: (img.width(), img.height()) != full,
            orientation_applied: true,
        }
    }
}

/// A developed picture.
#[derive(Debug, Clone)]
pub struct Developed {
    /// RGBA f16, row 0 = top, gamma encoded, unclamped; oriented upright.
    pub frame: FloatFrame,
    /// Size before any reduction (after orientation, so it has the frame's aspect).
    pub full_width: u32,
    pub full_height: u32,
    /// True when the frame is smaller than the full frame: box-downscaled to fit
    /// `max_edge`, or binned by `Quality::Preview`.
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
    // A camera profile that is not loaded (another machine, a deleted file) is left out.
    let dcp = settings
        .dcp
        .and_then(crate::dcp::get)
        .map(|d| d.prepare(as_shot_kelvin(raw), &m));
    let base_curve = dcp.as_ref().is_none_or(|p| !p.has_tone_curve());
    let to_srgb = |c: [f32; 3]| match &dcp {
        Some(p) => p.apply(c),
        None => color::apply(&m, c),
    };
    data.par_chunks_mut(3 * w as usize).for_each(|row| {
        for p in row.as_chunks_mut::<3>().0 {
            let balanced = [p[0] * wb[0], p[1] * wb[1], p[2] * wb[2]];
            let c = match highlights {
                Highlights::Clip => to_srgb(balanced.map(|v| v.min(1.0))).map(|v| v.min(1.0)),
                Highlights::Reconstruct => {
                    let mut c = to_srgb(balanced);
                    reconstruct(&mut c, clip_weight(p[0].max(p[1]).max(p[2])));
                    c
                }
            };
            for (out, v) in p.iter_mut().zip(c) {
                *out = if base_curve {
                    curve::apply(profile, v)
                } else {
                    v
                };
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
    // The full frame's size (the crop), also when Preview binned it smaller.
    let crop = demosaic::clamp_crop(raw);
    let (full_width, full_height) = oriented_size(o, crop.w, crop.h);
    let reduced = reduced || quality == Quality::Preview;
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

/// The temperature of `raw`'s as-shot white balance, from its colour matrix (`None` without
/// one): what a camera profile's two illuminants are blended by.
pub fn as_shot_kelvin(raw: &RawFrame) -> Option<f64> {
    let m = raw.xyz_to_cam?;
    if raw.wb[..3].iter().any(|v| !v.is_finite() || *v <= 0.0) {
        return None;
    }
    let neutral = [
        f64::from(raw.wb[1] / raw.wb[0]),
        1.0,
        f64::from(raw.wb[1] / raw.wb[2]),
    ];
    let xyz = color::solve3(m.map(|r| r.map(f64::from)), neutral)?;
    let sum = xyz[0] + xyz[1] + xyz[2];
    (sum.abs() > 1e-9).then(|| lf_settings::shader_values::kelvin_of_xy(xyz[0] / sum, xyz[1] / sum))
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

/// The 8-bit picture turned or flipped upright for EXIF orientation `o` (same index
/// maths as [`develop_rgb8`]); `img` itself when `o` is 1.
pub fn orient_rgba8(img: RgbaImage, o: Orientation) -> RgbaImage {
    if o.exif() == 1 {
        return img;
    }
    let (w, h) = img.dimensions();
    let (ow, oh) = oriented_size(o, w, h);
    let src = img.as_raw();
    let mut out = vec![0u8; ow as usize * oh as usize * 4];
    if ow > 0 {
        out.par_chunks_mut(ow as usize * 4)
            .enumerate()
            .for_each(|(oy, row)| {
                for (ox, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                    let (sx, sy) = source_of(o, w as usize, h as usize, ox, oy);
                    let i = (sy * w as usize + sx) * 4;
                    px.copy_from_slice(&src[i..i + 4]);
                }
            });
    }
    RgbaImage::from_raw(ow, oh, out).unwrap_or_else(|| RgbaImage::new(0, 0))
}

/// Integer box downscale of an 8-bit picture by `ceil(long_edge / max_edge)` (rounded
/// averages; partial edge blocks average the pixels they have). None when it already
/// fits.
pub fn box_downscale_rgba8(img: &RgbaImage, max_edge: u32) -> Option<RgbaImage> {
    let (w, h) = img.dimensions();
    let factor = w.max(h).div_ceil(max_edge.max(1));
    if factor <= 1 {
        return None;
    }
    let (nw, nh) = (w.div_ceil(factor), h.div_ceil(factor));
    let src = img.as_raw();
    let (wu, hu, f) = (w as usize, h as usize, factor as usize);
    let mut out = vec![0u8; nw as usize * nh as usize * 4];
    out.par_chunks_mut(nw as usize * 4)
        .enumerate()
        .for_each(|(oy, row)| {
            let (y0, y1) = (oy * f, ((oy + 1) * f).min(hu));
            for (ox, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let (x0, x1) = (ox * f, ((ox + 1) * f).min(wu));
                let mut sum = [0u32; 4];
                for y in y0..y1 {
                    for x in x0..x1 {
                        let i = (y * wu + x) * 4;
                        for c in 0..4 {
                            sum[c] += src[i + c] as u32;
                        }
                    }
                }
                let n = ((y1 - y0) * (x1 - x0)) as u32;
                for c in 0..4 {
                    px[c] = ((sum[c] + n / 2) / n) as u8;
                }
            }
        });
    RgbaImage::from_raw(nw, nh, out)
}

/// Camera level (fraction of white, before white balance) from which a channel
/// counts as clipped; the neutral blend ramps up over the last 2 %.
const CLIP_START: f32 = 0.98;

/// How far a pixel whose largest camera channel is `m` is pulled toward neutral.
#[inline]
fn clip_weight(m: f32) -> f32 {
    ((m - CLIP_START) / (1.0 - CLIP_START)).clamp(0.0, 1.0)
}

/// Blend a converted colour toward neutral by `t`, keeping its luminance.
#[inline]
fn reconstruct(c: &mut [f32; 3], t: f32) {
    if t <= 0.0 {
        return;
    }
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
