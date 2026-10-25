//! Setting groups for copy/paste and presets, ported from the web editor.

use serde_json::{Map, Value, json};

use crate::{EditSettings, SETTINGS_VERSION, normalise};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupKey {
    Light,
    Color,
    Hsl,
    Curve,
    Grading,
    Detail,
    Sharpen,
    Effects,
    Bw,
    Geometry,
    Spots,
}

/// Each group's label and the JSON keys it covers.
pub const GROUPS: [(GroupKey, &str, &[&str]); 11] = [
    (
        GroupKey::Light,
        "Light",
        &[
            "exposure",
            "contrast",
            "highlights",
            "shadows",
            "whites",
            "blacks",
        ],
    ),
    (
        GroupKey::Color,
        "White balance & colour",
        &["temperature", "tint", "vibrance", "saturation"],
    ),
    (GroupKey::Hsl, "Colour mix (HSL)", &["hsl"]),
    (GroupKey::Curve, "Tone curve", &["curve"]),
    (GroupKey::Grading, "Colour grading", &["grading"]),
    (
        GroupKey::Detail,
        "Clarity, texture & dehaze",
        &["clarity", "texture", "dehaze"],
    ),
    (
        GroupKey::Sharpen,
        "Sharpening & noise",
        &["sharpen", "sharpenRadius", "noise", "colorNoise"],
    ),
    (
        GroupKey::Effects,
        "Vignette & grain",
        &["vignette", "vignetteMidpoint", "grain", "grainSize"],
    ),
    (GroupKey::Bw, "Black & white", &["bw"]),
    (
        GroupKey::Geometry,
        "Crop, rotate & straighten",
        &["rotate", "flipH", "flipV", "angle", "crop", "aspect"],
    ),
    (GroupKey::Spots, "Spot removal", &["spots"]),
];

/// Copied by default: the look, not what belongs to one photo (its crop and spots).
pub const DEFAULT_COPY_GROUPS: [GroupKey; 9] = [
    GroupKey::Light,
    GroupKey::Color,
    GroupKey::Hsl,
    GroupKey::Curve,
    GroupKey::Grading,
    GroupKey::Detail,
    GroupKey::Sharpen,
    GroupKey::Effects,
    GroupKey::Bw,
];

fn keys_of(group: GroupKey) -> &'static [&'static str] {
    GROUPS
        .iter()
        .find(|(g, _, _)| *g == group)
        .map_or(&[], |(_, _, keys)| *keys)
}

/// The chosen groups of `from`, laid over `onto` (which keeps everything else).
pub fn apply_groups(onto: &EditSettings, from: &Value, groups: &[GroupKey]) -> EditSettings {
    let mut next = onto.to_value();
    if let Some(obj) = next.as_object_mut() {
        for group in groups {
            for key in keys_of(*group) {
                if let Some(v) = from.get(*key) {
                    obj.insert((*key).to_string(), v.clone());
                }
            }
        }
    }
    normalise(&next)
}

/// Only the chosen groups (what goes into a preset or on the clipboard).
pub fn pick_groups(s: &EditSettings, groups: &[GroupKey]) -> Value {
    let source = s.to_value();
    let mut out = Map::new();
    out.insert("v".to_string(), json!(SETTINGS_VERSION));
    for group in groups {
        for key in keys_of(*group) {
            if let Some(v) = source.get(*key) {
                out.insert((*key).to_string(), v.clone());
            }
        }
    }
    Value::Object(out)
}

/// The groups a partial settings object (preset, clipboard) actually changes.
pub fn groups_in(partial: &Value) -> Vec<GroupKey> {
    GROUPS
        .iter()
        .filter(|(_, _, keys)| keys.iter().any(|k| partial.get(*k).is_some()))
        .map(|(g, _, _)| *g)
        .collect()
}
