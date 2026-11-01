//! Imported Lightroom presets.
//!
//! An imported preset records where it came from under `desktop.lightroom`. That marker
//! changes how the preset is applied: Lightroom's presets change only the settings they
//! carry, whereas our own looks reset every look group they don't mention so that looks
//! don't pile up. The marker is in no settings group, so applying a preset never copies it
//! onto a photo.

use serde_json::Value;

pub mod read;

pub use read::{LrKind, LrPreset, ReadError, read_any, read_lrtemplate, read_xmp};

/// The key inside `desktop` that marks a preset as imported from Lightroom.
pub const LIGHTROOM_KEY: &str = "lightroom";

/// Whether `preset` is an imported Lightroom preset (it has `desktop.lightroom`).
pub fn is_lightroom(preset: &Value) -> bool {
    preset
        .get("desktop")
        .and_then(|d| d.get(LIGHTROOM_KEY))
        .is_some()
}
