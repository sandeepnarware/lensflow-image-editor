//! The built-in looks and how a look is applied, ported from the web editor.

use std::sync::LazyLock;

use serde_json::{Map, Value, json};

use crate::groups::{DEFAULT_COPY_GROUPS, GroupKey, apply_groups, groups_in};
use crate::{EditSettings, HSL_BANDS};

pub struct Look {
    pub id: &'static str,
    pub name: &'static str,
    /// A partial settings object.
    pub settings: Value,
}

fn curve(points: Value) -> Value {
    json!({"rgb": points, "r": [[0, 0], [1, 1]], "g": [[0, 0], [1, 1]], "b": [[0, 0], [1, 1]]})
}

fn grade(sh: Value, hi: Value) -> Value {
    json!({"shadows": sh, "midtones": {"hue": 30, "sat": 0}, "highlights": hi, "balance": 0})
}

fn hsl_look(bands: &[(&str, [i32; 3])]) -> Value {
    let mut out = Map::new();
    for band in HSL_BANDS {
        let name = format!("{band:?}").to_lowercase();
        let [h, s, l] = bands
            .iter()
            .find(|(n, _)| *n == name)
            .map_or([0, 0, 0], |(_, v)| *v);
        out.insert(name, json!({"h": h, "s": s, "l": l}));
    }
    Value::Object(out)
}

fn look(id: &'static str, name: &'static str, settings: Value) -> Look {
    Look { id, name, settings }
}

/// Starting points every studio gets; studios save their own as presets.
pub static LOOKS: LazyLock<[Look; 12]> = LazyLock::new(|| {
    [
        look(
            "natural",
            "Natural",
            json!({"contrast": 8, "highlights": -20, "shadows": 18, "vibrance": 15, "clarity": 6, "sharpen": 25}),
        ),
        look(
            "vivid",
            "Vivid",
            json!({"contrast": 22, "highlights": -15, "shadows": 10, "vibrance": 35, "saturation": 10, "clarity": 12, "dehaze": 6}),
        ),
        look(
            "warm",
            "Warm glow",
            json!({"temperature": 18, "tint": 4, "highlights": -12, "shadows": 12, "vibrance": 12, "vignette": -12}),
        ),
        look(
            "cool",
            "Cool",
            json!({"temperature": -16, "tint": -2, "contrast": 10, "vibrance": 8}),
        ),
        look(
            "golden",
            "Golden hour",
            json!({"temperature": 26, "tint": 8, "exposure": 0.1, "highlights": -25, "shadows": 20, "vibrance": 20,
                "grading": grade(json!({"hue": 220, "sat": 8}), json!({"hue": 40, "sat": 22})), "vignette": -15}),
        ),
        look(
            "matte",
            "Matte film",
            json!({"contrast": -12, "curve": curve(json!([[0, 0.08], [0.25, 0.25], [0.75, 0.76], [1, 0.95]])),
                "saturation": -12, "grain": 18,
                "grading": grade(json!({"hue": 200, "sat": 12}), json!({"hue": 45, "sat": 10}))}),
        ),
        look(
            "fade",
            "Soft fade",
            json!({"contrast": -18, "highlights": -10, "shadows": 25,
                "curve": curve(json!([[0, 0.1], [0.5, 0.52], [1, 0.97]])), "vibrance": -8}),
        ),
        look(
            "moody",
            "Moody",
            json!({"exposure": -0.25, "contrast": 18, "highlights": -35, "shadows": -10, "saturation": -15, "clarity": 15, "vignette": -30,
                "grading": grade(json!({"hue": 210, "sat": 18}), json!({"hue": 35, "sat": 8}))}),
        ),
        look(
            "teal-orange",
            "Teal & orange",
            json!({"contrast": 15, "vibrance": 15,
                "grading": grade(json!({"hue": 190, "sat": 30}), json!({"hue": 32, "sat": 28})),
                "hsl": hsl_look(&[("orange", [-4, 12, 6]), ("blue", [-12, 10, -6]), ("aqua", [8, 0, 0])])}),
        ),
        look(
            "bw",
            "B&W classic",
            json!({"bw": true, "contrast": 15, "clarity": 10}),
        ),
        look(
            "bw-high",
            "B&W high contrast",
            json!({"bw": true, "contrast": 40, "whites": 15, "blacks": -20, "clarity": 20, "grain": 12}),
        ),
        look(
            "bw-soft",
            "B&W soft",
            json!({"bw": true, "contrast": -10, "shadows": 20, "curve": curve(json!([[0, 0.06], [1, 0.96]])), "grain": 8}),
        ),
    ]
});

/// A look or preset applied to a photo: replaces the look groups it carries, keeps the crop and spots.
pub fn apply_look(onto: &EditSettings, look: &Value) -> EditSettings {
    let has_look_groups = groups_in(look)
        .iter()
        .any(|g| !matches!(g, GroupKey::Geometry | GroupKey::Spots));
    let defaults = EditSettings::default_settings().to_value();
    // A look resets the look groups it doesn't mention, so looks don't pile up on each other.
    let base = apply_groups(onto, &defaults, &DEFAULT_COPY_GROUPS);
    let mut merged = defaults;
    if let (Some(m), Some(l)) = (merged.as_object_mut(), look.as_object()) {
        for (k, v) in l {
            m.insert(k.clone(), v.clone());
        }
    }
    let groups: &[GroupKey] = if has_look_groups {
        &DEFAULT_COPY_GROUPS
    } else {
        &[]
    };
    apply_groups(&base, &merged, groups)
}
