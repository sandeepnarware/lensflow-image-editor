//! The develop settings model and its normalisation, ported from the web editor.

pub mod curve;
pub mod geometry;
mod normalise;
mod settings;
pub mod shader_values;

pub use normalise::normalise;
pub use settings::*;
