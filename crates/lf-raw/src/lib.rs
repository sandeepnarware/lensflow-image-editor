//! Decoding of camera RAW files into sensor data (via rawler), and developing it
//! into a picture ([`develop`]).

pub mod cache;
pub mod color;
pub mod curve;
pub mod decode;
pub mod demosaic;
pub mod develop;
mod error;
pub mod exif;
pub mod open;
pub mod render;

pub use error::RawError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Red,
    Green,
    Blue,
}

/// How the sensor samples colour.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cfa {
    Bayer {
        pattern: [[Color; 2]; 2],
    },
    XTrans {
        pattern: [[Color; 6]; 6],
    },
    /// Already demosaiced: three interleaved samples per pixel (e.g. linear DNG).
    Rgb,
}

/// Pixel rectangle: top-left corner and size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// EXIF orientation, 1..=8 (1 = upright).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Orientation(u8);

impl Orientation {
    pub fn from_exif(v: u16) -> Orientation {
        Orientation(if (1..=8).contains(&v) { v as u8 } else { 1 })
    }
    pub fn exif(self) -> u8 {
        self.0
    }
}

/// Undeveloped sensor data plus everything needed to develop it.
#[derive(Debug, Clone)]
pub struct RawFrame {
    pub width: u32,
    pub height: u32,
    pub cfa: Cfa,
    /// Row-major samples; `width * height` (Bayer, X-Trans) or `* 3` (Rgb).
    pub data: Vec<u16>,
    /// Black level per CFA position, in the 2x2 repeat's row-major order
    /// (top-left, top-right, bottom-left, bottom-right; RGGB gives [R, G1, G2, B]).
    /// For `Cfa::Rgb` frames the first three entries are per channel (R, G, B).
    pub black: [f32; 4],
    pub white: f32,
    /// As-shot white-balance multipliers per colour, [R, G, B, G2] (not per CFA
    /// position), green normalised to 1; G2 mirrors G when the file has none.
    pub wb: [f32; 4],
    /// The DNG `ColorMatrix` (XYZ to camera, a row per camera channel): the D65
    /// entry when the file has one.
    pub xyz_to_cam: Option<[[f32; 3]; 3]>,
    /// XYZ white point (Y = 1) of the illuminant `xyz_to_cam` was calibrated for
    /// ([`color::D65_WHITE`] when it is D65 or unknown); develop adapts it to D65.
    pub matrix_white: [f32; 3],
    pub crop: Rect,
    pub orientation: Orientation,
    pub make: String,
    pub model: String,
}
