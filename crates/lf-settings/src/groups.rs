//! Setting groups for copy/paste and presets, ported from the web editor and extended with
//! the desktop-only edits.
//!
//! A group's keys are top-level settings keys (`"exposure"`) or, for edits only the desktop
//! makes, `"desktop.<key>"` inside [`EditSettings::desktop`]. The web groups come first and
//! keep the web's meaning; the desktop groups were added after a paste carried saturation and
//! silently dropped the photo's calibration, lens corrections and masks.
//!
//! A desktop edit is absent when it is off, so a picked group records an absent edit as
//! `null`: pasting "Lens corrections" from an uncorrected photo then turns the target's off,
//! as Lightroom's paste does, instead of leaving them on.

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
    /// The RAW develop choices: profile, highlights, DNG camera profile.
    Raw,
    Negative,
    Lut,
    PointColour,
    Lens,
    Calibration,
    LensBlur,
    Masks,
    Geometry,
    Spots,
}

/// Where inside the settings JSON the desktop-only edits live.
const DESKTOP: &str = "desktop";

/// Each group's label and the keys it covers, in the order the picker shows them.
pub const GROUPS: [(GroupKey, &str, &[&str]); 19] = [
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
    (GroupKey::Raw, "RAW profile", &["desktop.develop"]),
    (GroupKey::Negative, "Film negative", &["desktop.negative"]),
    (GroupKey::Lut, "LUT look", &["desktop.lut"]),
    (
        GroupKey::PointColour,
        "Point colour",
        &["desktop.pointColour"],
    ),
    (
        GroupKey::Lens,
        "Lens corrections & defringe",
        &["desktop.lens", "desktop.defringe"],
    ),
    (
        GroupKey::Calibration,
        "Calibration",
        &["desktop.calibration"],
    ),
    (GroupKey::LensBlur, "Lens blur", &["desktop.lensBlur"]),
    (GroupKey::Masks, "Masking", &["desktop.masks"]),
    (
        GroupKey::Geometry,
        "Crop, rotate & straighten",
        &["rotate", "flipH", "flipV", "angle", "crop", "aspect"],
    ),
    (
        GroupKey::Spots,
        "Spot removal & healing",
        &["spots", "desktop.heal"],
    ),
];

/// The web's look groups: what a look or studio preset resets before it is laid on, and
/// what "Match a reference" grades. Not the photo's crop and spots.
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

/// Ticked when the copy or save-preset dialog first opens: the look, desktop edits included,
/// but not what belongs to one photo (crop, spots and healing, masks, lens blur's depth).
pub const COPY_DEFAULT_GROUPS: [GroupKey; 15] = [
    GroupKey::Light,
    GroupKey::Color,
    GroupKey::Hsl,
    GroupKey::Curve,
    GroupKey::Grading,
    GroupKey::Detail,
    GroupKey::Sharpen,
    GroupKey::Effects,
    GroupKey::Bw,
    GroupKey::Raw,
    GroupKey::Negative,
    GroupKey::Lut,
    GroupKey::PointColour,
    GroupKey::Lens,
    GroupKey::Calibration,
];

fn keys_of(group: GroupKey) -> &'static [&'static str] {
    GROUPS
        .iter()
        .find(|(g, _, _)| *g == group)
        .map_or(&[], |(_, _, keys)| *keys)
}

/// A desktop key's name inside the `desktop` object, or `None` for a top-level key.
fn desktop_key(key: &str) -> Option<&str> {
    key.strip_prefix("desktop.")
}

/// The value `key` holds in a settings JSON object (`null` included).
fn lookup<'a>(from: &'a Value, key: &str) -> Option<&'a Value> {
    match desktop_key(key) {
        Some(k) => from.get(DESKTOP)?.get(k),
        None => from.get(key),
    }
}

/// The `desktop` object of `obj`, made when missing (or not an object).
fn desktop_mut(obj: &mut Map<String, Value>) -> &mut Map<String, Value> {
    let entry = obj
        .entry(DESKTOP)
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    entry.as_object_mut().expect("just made an object")
}

/// The chosen groups of `from`, laid over `onto` (which keeps everything else). A desktop
/// edit that `from` carries as `null` is turned off on `onto`; one it doesn't carry is left.
pub fn apply_groups(onto: &EditSettings, from: &Value, groups: &[GroupKey]) -> EditSettings {
    let mut next = onto.to_value();
    if let Some(obj) = next.as_object_mut() {
        for group in groups {
            for key in keys_of(*group) {
                let Some(v) = lookup(from, key) else {
                    continue;
                };
                match desktop_key(key) {
                    Some(k) if v.is_null() => {
                        if let Some(d) = obj.get_mut(DESKTOP).and_then(Value::as_object_mut) {
                            d.remove(k);
                        }
                    }
                    Some(k) => {
                        desktop_mut(obj).insert(k.to_string(), v.clone());
                    }
                    None => {
                        obj.insert((*key).to_string(), v.clone());
                    }
                }
            }
        }
    }
    // `normalise` drops a `desktop` object left empty.
    normalise(&next)
}

/// Only the chosen groups (what goes into a preset or on the clipboard). A desktop edit the
/// photo doesn't have is recorded as `null`, so pasting it turns the target's off.
pub fn pick_groups(s: &EditSettings, groups: &[GroupKey]) -> Value {
    let source = s.to_value();
    let mut out = Map::new();
    out.insert("v".to_string(), json!(SETTINGS_VERSION));
    for group in groups {
        for key in keys_of(*group) {
            match desktop_key(key) {
                Some(k) => {
                    let v = lookup(&source, key).cloned().unwrap_or(Value::Null);
                    desktop_mut(&mut out).insert(k.to_string(), v);
                }
                None => {
                    if let Some(v) = source.get(*key) {
                        out.insert((*key).to_string(), v.clone());
                    }
                }
            }
        }
    }
    Value::Object(out)
}

/// Takes `groups` out of a partial settings object (a batch paste keeps each photo's own
/// crop, spots and healing). A `desktop` object left empty is removed.
pub fn drop_groups(partial: &mut Value, groups: &[GroupKey]) {
    let Some(obj) = partial.as_object_mut() else {
        return;
    };
    for group in groups {
        for key in keys_of(*group) {
            match desktop_key(key) {
                Some(k) => {
                    if let Some(d) = obj.get_mut(DESKTOP).and_then(Value::as_object_mut) {
                        d.remove(k);
                    }
                }
                None => {
                    obj.remove(*key);
                }
            }
        }
    }
    if obj
        .get(DESKTOP)
        .and_then(Value::as_object)
        .is_some_and(Map::is_empty)
    {
        obj.remove(DESKTOP);
    }
}

/// The groups a partial settings object (preset, clipboard) actually changes.
pub fn groups_in(partial: &Value) -> Vec<GroupKey> {
    GROUPS
        .iter()
        .filter(|(_, _, keys)| keys.iter().any(|k| lookup(partial, k).is_some()))
        .map(|(g, _, _)| *g)
        .collect()
}

/// The desktop groups among `groups` (those whose keys live in `desktop`).
pub fn desktop_groups(groups: &[GroupKey]) -> Vec<GroupKey> {
    groups
        .iter()
        .copied()
        .filter(|g| keys_of(*g).iter().all(|k| desktop_key(k).is_some()))
        .collect()
}
