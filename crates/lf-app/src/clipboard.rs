//! Copy and paste of settings (`Ctrl+Shift+C` / `Ctrl+Shift+V`), as the web's `state.ts`.
//!
//! Copy keeps the chosen groups of the photo's settings (`pick_groups`) in an in-app slot
//! and puts the same JSON on the system clipboard (`ctx.copy_text`), so it can be pasted
//! into another copy of the app or kept in a note. Paste uses the slot; when the slot is
//! empty it reads the system clipboard (egui answers a paste request with the text) and
//! uses it when it reads as settings.

use crate::develop::debounce::SidecarSaver;
use lf_catalog::Catalog;
use lf_settings::groups::{GROUPS, GroupKey, apply_groups, groups_in, pick_groups};
use lf_settings::looks::apply_look;
use lf_settings::{EditSettings, normalise};
use serde_json::{Map, Value};

/// The toast after a copy.
pub const COPIED: &str = "Edits copied. Paste them on another photo, or on many selected photos.";
/// The toast when the clipboard holds nothing to paste.
pub const NOTHING: &str = "Nothing to paste: copy a photo's edits first (Ctrl+Shift+C).";

/// The toast when the system clipboard holds far more text than any edits.
pub const TOO_LONG: &str = "The clipboard holds too much text to be copied edits.";
/// The longest clipboard text read as edits.
pub const MAX_TEXT: usize = 256 * 1024;

/// How settings are laid over a photo.
#[derive(Debug, Clone, PartialEq)]
pub enum Recipe {
    /// Copied edits: the groups they carry replace the photo's (the web's paste).
    Paste(Value),
    /// A look or studio preset (`apply_look`, the web's `applyLook`): the look groups are put
    /// back to their defaults, then the preset is laid on; its crop, rotation and spots are
    /// never applied.
    Look(Value),
    /// Auto-sync: the keys one gesture changed on another photo ([`crate::sync`]).
    Sync(Value),
    /// A quarter turn (the Library's Rotate left or right): the crop turns with the photo.
    Turn(lf_settings::geometry::TurnDir),
}

impl Recipe {
    /// The settings `onto` becomes.
    pub fn apply(&self, onto: &EditSettings) -> EditSettings {
        match self {
            Recipe::Paste(partial) => apply(onto, partial),
            Recipe::Look(look) => apply_look(onto, &clipboard_settings(look)),
            Recipe::Sync(patch) => crate::sync::apply(onto, patch),
            Recipe::Turn(dir) => {
                let mut s = onto.clone();
                crate::develop::tools::crop::turn(&mut s, *dir);
                s
            }
        }
    }

    /// The same without crop, rotation, straighten or spots, so each photo keeps its own
    /// (applying to many photos).
    pub fn without_geometry(self) -> Recipe {
        match self {
            Recipe::Paste(mut partial) => {
                if let Some(m) = partial.as_object_mut() {
                    for (group, _, keys) in GROUPS {
                        if matches!(group, GroupKey::Geometry | GroupKey::Spots) {
                            for key in keys {
                                m.remove(*key);
                            }
                        }
                    }
                }
                Recipe::Paste(partial)
            }
            look => look,
        }
    }
}

/// The in-app slot.
#[derive(Debug, Clone, Default)]
pub struct Clipboard {
    slot: Option<Value>,
}

impl Clipboard {
    /// Keeps `groups` of `settings`; returns the JSON text for the system clipboard.
    pub fn copy(&mut self, settings: &EditSettings, groups: &[GroupKey]) -> String {
        let picked = pick_groups(settings, groups);
        let text = picked.to_string();
        self.slot = Some(picked);
        text
    }

    /// What was copied last in this app.
    pub fn slot(&self) -> Option<&Value> {
        self.slot.as_ref()
    }

    /// What a paste applies: the slot, else `system` (the system clipboard's text) when it
    /// reads as settings.
    pub fn source(&self, system: Option<&str>) -> Option<Value> {
        self.slot.clone().or_else(|| system.and_then(parse))
    }
}

/// Settings copied as text: a JSON object carrying at least one setting group, made safe
/// ([`clipboard_settings`]). Anything else is `None`.
pub fn parse(text: &str) -> Option<Value> {
    if text.len() > MAX_TEXT {
        return None;
    }
    let value: Value = serde_json::from_str(text.trim()).ok()?;
    (value.is_object() && !groups_in(&value).is_empty()).then(|| clipboard_settings(&value))
}

/// Pasted or preset settings made safe: only the keys they carry, each normalised (clamped
/// to its range), as the web's `clipboardSettings`.
pub fn clipboard_settings(partial: &Value) -> Value {
    let Some(carried) = partial.as_object() else {
        return Value::Object(Map::new());
    };
    let mut merged = EditSettings::default_settings().to_value();
    if let Some(m) = merged.as_object_mut() {
        for (k, v) in carried {
            m.insert(k.clone(), v.clone());
        }
    }
    let full = normalise(&merged).to_value();
    let out: Map<String, Value> = carried
        .keys()
        .filter_map(|k| full.get(k).map(|v| (k.clone(), v.clone())))
        .collect();
    Value::Object(out)
}

/// `partial` laid over `onto`: the groups it carries replace the photo's, the rest stays.
pub fn apply(onto: &EditSettings, partial: &Value) -> EditSettings {
    apply_groups(onto, &clipboard_settings(partial), &groups_in(partial))
}

/// Pastes `partial` onto each of `photo_ids`: one `set_settings` and one sidecar write per
/// photo (none for a photo whose sidecar is damaged). Each photo's outcome, in order.
pub fn paste(
    catalog: &Catalog,
    photo_ids: &[i64],
    partial: &Value,
    saver: &SidecarSaver,
) -> Vec<(i64, Result<(), String>)> {
    let recipe = Recipe::Paste(partial.clone());
    photo_ids
        .iter()
        .map(|&id| {
            (
                id,
                crate::dialogs::batch::apply_one(catalog, id, &recipe, saver),
            )
        })
        .collect()
}

/// `pick_groups`, for the copy dialog and presets.
pub fn picked(settings: &EditSettings, groups: &[GroupKey]) -> Value {
    pick_groups(settings, groups)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lf_catalog::{NewPhoto, edits, sidecar};
    use serde_json::json;

    #[test]
    fn copy_picks_only_chosen_groups() {
        let mut s = EditSettings::default_settings();
        s.exposure = 1.0;
        s.temperature = 10.0;
        s.bw = true;
        s.crop.w = 0.5;
        let mut clip = Clipboard::default();
        let text = clip.copy(&s, &[GroupKey::Light, GroupKey::Bw]);
        let slot = clip.slot().unwrap();
        assert_eq!(slot["exposure"].as_f64(), Some(1.0));
        assert_eq!(slot["bw"], json!(true));
        assert!(slot.get("temperature").is_none(), "{slot}");
        assert!(slot.get("crop").is_none(), "{slot}");
        assert_eq!(
            groups_in(slot),
            [GroupKey::Light, GroupKey::Bw],
            "only the chosen groups"
        );
        // The text on the system clipboard reads back as the same settings.
        let back = parse(&text).unwrap();
        assert_eq!(groups_in(&back), [GroupKey::Light, GroupKey::Bw]);
        assert_eq!(back["exposure"].as_f64(), Some(1.0));
    }

    #[test]
    fn only_settings_text_pastes_and_values_are_clamped() {
        assert!(parse("hello").is_none());
        assert!(parse("[1, 2]").is_none());
        assert!(parse(r#"{"name": "not settings"}"#).is_none());
        let v = parse(r#"{"exposure": 99, "nonsense": 1}"#).unwrap();
        assert_eq!(v["exposure"].as_f64(), Some(5.0), "clamped to its range");
        assert!(v.get("nonsense").is_none());
        assert!(v.get("contrast").is_none(), "only the keys it carries");
        // The slot wins over the system clipboard; an empty slot reads it.
        let empty = Clipboard::default();
        assert!(empty.source(Some("hello")).is_none());
        assert_eq!(
            empty.source(Some(r#"{"exposure": 1}"#)).unwrap()["exposure"].as_f64(),
            Some(1.0)
        );
    }

    #[test]
    fn a_preset_applies_as_the_web_applies_a_look() {
        let mut onto = EditSettings::default_settings();
        onto.exposure = 1.0;
        onto.temperature = 25.0;
        onto.crop.w = 0.5;
        onto.angle = 3.0;
        // No Light group in the preset: exposure goes back to 0 (the look groups reset).
        let next = Recipe::Look(json!({"v": 2, "contrast": 20})).apply(&onto);
        assert_eq!(
            (next.exposure, next.contrast, next.temperature),
            (0.0, 20.0, 0.0)
        );
        assert_eq!((next.crop, next.angle), (onto.crop, 3.0), "geometry kept");
        // A preset carrying a crop and spots never moves the photo's.
        let preset = json!({"v": 2, "exposure": 0.5, "crop": {"x": 0.1, "y": 0.1, "w": 0.2, "h": 0.2},
                            "angle": 10, "spots": [{"x": 0.5, "y": 0.5, "r": 0.02, "sx": 0.6, "sy": 0.5}]});
        let next = Recipe::Look(preset).apply(&onto);
        assert_eq!(next.exposure, 0.5);
        assert_eq!((next.crop, next.angle), (onto.crop, 3.0));
        assert!(next.spots.is_empty());
    }

    #[test]
    fn a_batch_paste_keeps_each_photos_crop_and_spots() {
        let mut onto = EditSettings::default_settings();
        onto.crop.w = 0.5;
        let copied = json!({"v": 2, "exposure": 1, "crop": {"x": 0, "y": 0, "w": 1, "h": 1},
                            "angle": 5, "spots": []});
        let one = Recipe::Paste(copied.clone()).apply(&onto);
        assert_eq!(
            (one.crop.w, one.angle),
            (1.0, 5.0),
            "one photo takes all it was given"
        );
        let many = Recipe::Paste(copied).without_geometry().apply(&onto);
        assert_eq!(many.exposure, 1.0);
        assert_eq!((many.crop, many.angle), (onto.crop, 0.0));
    }

    #[test]
    fn huge_clipboard_text_is_not_read() {
        let mut text = String::from("{\"exposure\": 1, \"x\": \"");
        text.push_str(&"a".repeat(MAX_TEXT));
        text.push_str("\"}");
        assert!(parse(&text).is_none());
        assert!(parse(r#"{"exposure": 1}"#).is_some());
    }

    #[test]
    fn paste_applies_to_each_selected() {
        let dir = tempfile::tempdir().unwrap();
        let c = Catalog::open(dir.path()).unwrap();
        let folder = c.add_folder(&dir.path().to_string_lossy()).unwrap();
        let ids: Vec<i64> = ["a.jpg", "b.jpg", "c.jpg"]
            .iter()
            .map(|name| {
                let path = dir.path().join(name);
                std::fs::write(&path, b"x").unwrap();
                c.add_photo(
                    folder,
                    &NewPhoto {
                        path: path.to_string_lossy().into_owned(),
                        kind: "jpeg".into(),
                        ..Default::default()
                    },
                )
                .unwrap()
            })
            .collect();
        // The first photo has its own white balance, which a Light paste keeps.
        let mut warm = EditSettings::default_settings();
        warm.temperature = 30.0;
        edits::set_settings(&c, ids[0], &warm).unwrap();

        let mut clip = Clipboard::default();
        let mut source = EditSettings::default_settings();
        source.exposure = 0.8;
        source.temperature = -50.0;
        clip.copy(&source, &[GroupKey::Light]);
        let saver = SidecarSaver::new();
        let selected = [ids[0], ids[2]];
        let done = paste(&c, &selected, clip.slot().unwrap(), &saver);
        assert_eq!(done.len(), 2);
        assert!(done.iter().all(|(_, r)| r.is_ok()), "{done:?}");

        let get = |id| edits::get_settings(&c, id).unwrap();
        let first = get(ids[0]).unwrap();
        assert_eq!((first.exposure, first.temperature), (0.8, 30.0));
        assert_eq!(get(ids[2]).unwrap().exposure, 0.8);
        assert!(get(ids[1]).is_none(), "the photo not selected is untouched");
        // Each pasted photo's sidecar is written beside it, at once.
        for (name, written) in [("a.jpg", true), ("b.jpg", false), ("c.jpg", true)] {
            let path = sidecar::path_for(&dir.path().join(name));
            assert_eq!(path.exists(), written, "{name}");
            if written {
                let sc = sidecar::read(&path).unwrap().unwrap();
                assert_eq!(sc.settings.exposure, 0.8);
            }
        }
    }
}
