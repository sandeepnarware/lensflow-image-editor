//! Auto-sync: what one gesture changed on the open photo, to make the same change on the
//! other selected photos (Lightroom's Auto Sync). [`changes`] compares the photo's settings
//! before and after a commit key by key (inside `desktop` too, so desktop-only edits such as
//! lens corrections, calibration or a LUT follow), and [`apply`] lays those keys over another
//! photo's settings, leaving everything else of that photo alone.
//!
//! What belongs to one photo is never synced: its crop, turn and straightening, its spots and
//! healing strokes, and its masks (placed on that photo, or found in it by AI).

use lf_settings::{EditSettings, normalise};
use serde_json::{Map, Value};

/// Keys that belong to one photo.
const OWN: [&str; 7] = [
    "crop", "angle", "rotate", "flipH", "flipV", "aspect", "spots",
];
/// `desktop` keys that belong to one photo.
const OWN_DESKTOP: [&str; 2] = ["heal", "masks"];

/// What changed from `before` to `after` (a key set to `null` was taken away), or `None` when
/// nothing that syncs did.
pub fn changes(before: &EditSettings, after: &EditSettings) -> Option<Value> {
    let (b, a) = (before.to_value(), after.to_value());
    let (Some(b), Some(a)) = (b.as_object(), a.as_object()) else {
        return None;
    };
    let mut out = Map::new();
    let mut keys: Vec<&String> = b.keys().chain(a.keys()).collect();
    keys.sort();
    keys.dedup();
    for key in keys {
        if OWN.contains(&key.as_str()) {
            continue;
        }
        let (bv, av) = (b.get(key), a.get(key));
        if key == "desktop" {
            let empty = Map::new();
            let bd = bv.and_then(Value::as_object).unwrap_or(&empty);
            let ad = av.and_then(Value::as_object).unwrap_or(&empty);
            let mut d = Map::new();
            for k in bd.keys().chain(ad.keys()) {
                if OWN_DESKTOP.contains(&k.as_str()) || d.contains_key(k) {
                    continue;
                }
                if bd.get(k) != ad.get(k) {
                    d.insert(k.clone(), ad.get(k).cloned().unwrap_or(Value::Null));
                }
            }
            if !d.is_empty() {
                out.insert(key.clone(), Value::Object(d));
            }
        } else if bv != av {
            out.insert(key.clone(), av.cloned().unwrap_or(Value::Null));
        }
    }
    (!out.is_empty()).then_some(Value::Object(out))
}

/// `later` laid over `earlier` (two gestures not yet synced become one patch).
pub fn merge(earlier: &mut Value, later: Value) {
    let (Some(e), Value::Object(l)) = (earlier.as_object_mut(), later) else {
        return;
    };
    for (k, v) in l {
        match (e.get_mut(&k), v) {
            (Some(Value::Object(ed)), Value::Object(ld)) if k == "desktop" => {
                ed.extend(ld);
            }
            (_, v) => {
                e.insert(k, v);
            }
        }
    }
}

/// `onto` with `patch` ([`changes`]) laid over it.
pub fn apply(onto: &EditSettings, patch: &Value) -> EditSettings {
    let mut v = onto.to_value();
    let (Some(obj), Some(p)) = (v.as_object_mut(), patch.as_object()) else {
        return onto.clone();
    };
    for (k, val) in p {
        if k == "desktop" {
            let Some(pd) = val.as_object() else {
                continue;
            };
            let d = obj
                .entry("desktop")
                .or_insert_with(|| Value::Object(Map::new()));
            if !d.is_object() {
                *d = Value::Object(Map::new());
            }
            let d = d.as_object_mut().expect("an object");
            for (dk, dv) in pd {
                if dv.is_null() {
                    d.remove(dk);
                } else {
                    d.insert(dk.clone(), dv.clone());
                }
            }
            if d.is_empty() {
                obj.remove("desktop");
            }
        } else if val.is_null() {
            obj.remove(k);
        } else {
            obj.insert(k.clone(), val.clone());
        }
    }
    normalise(&v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lf_settings::calibration::Calibration;
    use lf_settings::lens::Lens;

    #[test]
    fn only_what_changed_is_synced_and_the_rest_of_the_other_photo_stays() {
        let before = EditSettings::default_settings();
        let mut after = before.clone();
        after.exposure = 0.7;
        Calibration {
            red_hue: 20.0,
            ..Calibration::default()
        }
        .write_into(&mut after);
        let patch = changes(&before, &after).unwrap();
        assert_eq!(patch["exposure"], serde_json::json!(0.7));
        assert!(patch.get("contrast").is_none());
        assert!(patch["desktop"]["calibration"].is_object());

        // Another photo with its own contrast and lens correction keeps them.
        let mut other = EditSettings::default_settings();
        other.contrast = 30.0;
        Lens {
            vertical: 10.0,
            ..Lens::default()
        }
        .write_into(&mut other);
        let synced = apply(&other, &patch);
        assert_eq!(synced.exposure, 0.7);
        assert_eq!(synced.contrast, 30.0);
        assert_eq!(Lens::from_settings(&synced).vertical, 10.0);
        assert_eq!(Calibration::from_settings(&synced).red_hue, 20.0);
    }

    #[test]
    fn a_removed_desktop_edit_is_removed_from_the_others_too() {
        let mut before = EditSettings::default_settings();
        Calibration {
            blue_saturation: 40.0,
            ..Calibration::default()
        }
        .write_into(&mut before);
        let after = EditSettings::default_settings();
        let patch = changes(&before, &after).unwrap();
        let synced = apply(&before, &patch);
        assert!(Calibration::from_settings(&synced).is_neutral());
        assert_eq!(synced.desktop, None);
    }

    #[test]
    fn what_belongs_to_one_photo_never_syncs() {
        let before = EditSettings::default_settings();
        let mut after = before.clone();
        after.angle = 3.0;
        after.crop.w = 0.5;
        assert_eq!(changes(&before, &after), None);
        let mut masked = before.clone();
        masked.desktop = Some(
            serde_json::from_value(serde_json::json!({"masks": [{"kind": "radial"}]})).unwrap(),
        );
        assert_eq!(changes(&before, &masked), None);
    }

    #[test]
    fn gestures_merge_later_first() {
        let mut a = serde_json::json!({"exposure": 1, "desktop": {"lut": {"name": "A"}}});
        merge(
            &mut a,
            serde_json::json!({"exposure": 2, "desktop": {"calibration": {"redHue": 5}}}),
        );
        assert_eq!(a["exposure"], 2);
        assert_eq!(a["desktop"]["lut"]["name"], "A");
        assert_eq!(a["desktop"]["calibration"]["redHue"], 5);
    }
}
