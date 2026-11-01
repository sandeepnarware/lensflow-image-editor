//! Keeping an imported preset's `desktop` under the cap.
//!
//! `normalise` drops a `desktop` object larger than `MAX_DESKTOP_JSON` whole, which would lose
//! the preset's edits with it. The one big part we can spare is the copy of Lightroom's own
//! settings under `desktop.lightroom.settings` (kept only so a better mapping can translate the
//! preset again), so that goes first and the user is told.

use serde_json::Value;

use super::LIGHTROOM_KEY;
use crate::MAX_DESKTOP_JSON;

pub const BUDGET_NOTE: &str = "This preset is large, so Lightroom's original settings were not \
     kept with it; what was carried is unaffected.";

fn desktop_len(preset: &Value) -> usize {
    preset
        .get("desktop")
        .and_then(|d| serde_json::to_vec(d).ok())
        .map_or(0, |b| b.len())
}

/// Drops `desktop.lightroom.settings` when `desktop` is over the cap; returns the note when it
/// did. Everything else in `desktop` is left as it is.
pub fn fit_budget(preset: &mut Value) -> Option<&'static str> {
    if desktop_len(preset) <= MAX_DESKTOP_JSON {
        return None;
    }
    let lr = preset
        .get_mut("desktop")?
        .get_mut(LIGHTROOM_KEY)?
        .as_object_mut()?;
    lr.remove("settings")?;
    Some(BUDGET_NOTE)
}
