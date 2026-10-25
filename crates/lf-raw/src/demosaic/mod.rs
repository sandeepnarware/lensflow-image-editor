//! Demosaicing: turn single-colour sensor samples into full RGB per pixel.
//!
//! Output is linear camera-space RGB with the black level subtracted (per CFA
//! position) and divided by `white - black`, so 0..1 is the sensor's range. Values
//! are not clamped above 1 (nor below 0, which noise around black can produce).
//! White balance is not applied here. The output covers the frame's `crop`
//! rectangle only; samples outside it still feed the interpolation at its edges.
//!
//! - `Quality::Fast`: bilinear ([`bilinear`]), Bayer and X-Trans.
//! - `Quality::Best`: AHD for Bayer ([`ahd`]); a provisional inverse-distance
//!   interpolation for X-Trans ([`xtrans`]).

mod ahd;
mod bilinear;
mod xtrans;

use crate::{Cfa, Color, RawFrame, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    /// Bilinear: quick, softer, zipper artefacts on hard edges.
    Fast,
    /// AHD for Bayer; 3-colour inverse-distance interpolation for X-Trans.
    Best,
}

/// Linear camera-space RGB, interleaved, row-major.
#[derive(Debug, Clone)]
pub struct LinearRgb {
    pub width: u32,
    pub height: u32,
    pub data: Vec<f32>,
}

/// Demosaic `raw` at the given quality (parallel over rows with rayon).
pub fn run(raw: &RawFrame, quality: Quality) -> LinearRgb {
    let crop = clamp_crop(raw);
    let data = match &raw.cfa {
        Cfa::Rgb => pass_through(raw, crop),
        Cfa::Bayer { pattern } => {
            let plane = Plane::normalise(raw);
            let pattern = pattern.map(|row| row.map(index));
            match quality {
                Quality::Fast => bilinear::bayer(&plane, pattern, crop),
                Quality::Best => ahd::run(&plane, pattern, crop),
            }
        }
        Cfa::XTrans { pattern } => {
            let plane = Plane::normalise(raw);
            let pattern = pattern.map(|row| row.map(index));
            match quality {
                Quality::Fast => xtrans::run(&plane, &pattern, crop, xtrans::Weighting::Mean3x3),
                Quality::Best => xtrans::run(
                    &plane,
                    &pattern,
                    crop,
                    xtrans::Weighting::InverseDistance5x5,
                ),
            }
        }
    };
    LinearRgb {
        width: crop.w,
        height: crop.h,
        data,
    }
}

/// Channel index of a colour: R = 0, G = 1, B = 2.
fn index(c: Color) -> usize {
    match c {
        Color::Red => 0,
        Color::Green => 1,
        Color::Blue => 2,
    }
}

fn clamp_crop(raw: &RawFrame) -> Rect {
    let x = raw.crop.x.min(raw.width);
    let y = raw.crop.y.min(raw.height);
    Rect {
        x,
        y,
        w: raw.crop.w.min(raw.width - x),
        h: raw.crop.h.min(raw.height - y),
    }
}

fn pass_through(raw: &RawFrame, crop: Rect) -> Vec<f32> {
    let w = raw.width as usize;
    let mut out = Vec::with_capacity(crop.w as usize * crop.h as usize * 3);
    for y in crop.y as usize..(crop.y + crop.h) as usize {
        for x in crop.x as usize..(crop.x + crop.w) as usize {
            for c in 0..3 {
                let black = raw.black[c];
                let v = raw.data[(y * w + x) * 3 + c] as f32;
                out.push((v - black) / (raw.white - black));
            }
        }
    }
    out
}

/// The mosaic as normalised f32 samples (black subtracted per 2x2 position).
pub(crate) struct Plane {
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

impl Plane {
    fn normalise(raw: &RawFrame) -> Plane {
        let (w, h) = (raw.width as usize, raw.height as usize);
        let scale = raw.black.map(|b| 1.0 / (raw.white - b));
        let data = raw.data[..w * h]
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let pos = ((i / w) & 1) * 2 + ((i % w) & 1);
                (v as f32 - raw.black[pos]) * scale[pos]
            })
            .collect();
        Plane { w, h, data }
    }

    /// Sample at (x, y), mirrored at the edges (`-1` reads `1`, `w` reads `w - 2`).
    /// Mirroring keeps the 2x2 CFA phase, so the colour at an out-of-range
    /// coordinate is the colour its parity says.
    #[inline]
    pub fn mirrored(&self, x: isize, y: isize) -> f32 {
        self.data[mirror(y, self.h) * self.w + mirror(x, self.w)]
    }
}

#[inline]
pub(crate) fn mirror(i: isize, n: usize) -> usize {
    let last = n as isize - 1;
    let i = if i < 0 { -i } else { i };
    let i = if i > last { 2 * last - i } else { i };
    i.clamp(0, last) as usize
}
