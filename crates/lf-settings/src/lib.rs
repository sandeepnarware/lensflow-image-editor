//! The develop settings model and its normalisation, ported from the web editor.
//!
//! **Entry point for JSON:** [`normalise`] (or [`EditSettings::from_json_str`] for text) is
//! the only way to read untrusted JSON: anything saved by the web, pasted, or from a preset.
//! It drops unknown keys, defaults missing ones and clamps every number, as the web does.
//! The derived `Deserialize` on [`EditSettings`] is strict (a missing field, or `90.0` for
//! `rotate`, is an error) and is only for JSON that is already normal, such as our own
//! [`EditSettings::to_value`] output.

pub mod curve;
pub mod geometry;
pub mod groups;
pub mod heal;
pub mod lens;
pub mod looks;
pub mod lut;
pub mod masks;
mod normalise;
mod settings;
pub mod shader_values;

pub use normalise::normalise;
pub use settings::*;
