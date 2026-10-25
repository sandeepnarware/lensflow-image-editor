//! Matching a reference image's colour grade: pure CPU maths ported from the web editor.
//!
//! * [`measure`] reads pixel statistics of an image.
//! * [`correct`] is one calibration round, using the inverse of the shader's formulas.
//! * [`calibrate`] loops rounds with an injected renderer.
//! * [`apply_strength`] blends a grade towards neutral; [`exposure_offset`] matches brightness.
//! * [`sheet`] picks sample photos and tiles them into the contact sheet.
//!
//! Luma is 0.2126 R + 0.7152 G + 0.0722 B on gamma values, as the shader's.

mod calibrate;
mod correct;
mod measure;
pub mod sheet;
mod strength;
mod util;

pub use calibrate::{
    CHROMA_THRESHOLD, CalibrateOptions, Calibration, LUMA_THRESHOLD, calibrate, preserve,
};
pub use correct::{CorrectOptions, Correction, Residual, correct, tidy_curve};
pub use measure::{
    BandStats, PERCENTILES, Stats, measure, wheel_from_offset, white_balance_from_gains,
};
pub use strength::{apply_strength, exposure_offset};
pub use util::normalise_settings;
