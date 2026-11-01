//! Imported Lightroom presets.
//!
//! An imported preset records where it came from under `desktop.lightroom`. That marker
//! changes how the preset is applied: Lightroom's presets change only the settings they
//! carry, whereas our own looks reset every look group they don't mention so that looks
//! don't pile up. The marker is in no settings group, so applying a preset never copies it
//! onto a photo.

use serde_json::Value;

pub mod budget;
mod curve;
pub mod map;
pub mod read;

pub use map::{Mapped, map};
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

/// Whether an imported preset asks for Auto white balance and Auto tone (`desktop.lightroom.auto`),
/// which are worked out from each photo when the preset is applied, never stored in the preset.
pub fn auto_flags(preset: &Value) -> (bool, bool) {
    let flag = |key: &str| {
        preset
            .get("desktop")
            .and_then(|d| d.get(LIGHTROOM_KEY))
            .and_then(|l| l.get("auto"))
            .and_then(|a| a.get(key))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    (flag("wb"), flag("tone"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn auto_flags_read_the_marker() {
        let p = json!({"desktop": {"lightroom": {"auto": {"wb": true, "tone": false}}}});
        assert_eq!(auto_flags(&p), (true, false));
        assert_eq!(auto_flags(&json!({"v": 2})), (false, false));
        assert_eq!(
            auto_flags(&json!({"desktop": {"lightroom": {}}})),
            (false, false)
        );
    }
}
