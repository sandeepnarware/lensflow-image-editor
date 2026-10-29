//! Adaptive presets: a studio preset may carry its photo's AI masks (sky, subject, people and
//! their parts, near and far) not as the shapes found in that photo but as what to find
//! (`Shape::Image::what`) with the mask's own settings. Applying the preset to another photo
//! finds each one again there and adds it with those settings, so "darken the sky, brighten
//! the subject" follows each photo.
//!
//! They live in the preset's JSON under `desktop.adaptiveMasks`, which the web editor's
//! `applyLook` (groups only) never reads; applying a preset as a look leaves them out of the
//! photo's own settings.

use crate::masks::{Local, Mask, Masks, Range, Shape};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The key under the preset's `desktop`.
pub const KEY: &str = "adaptiveMasks";

/// One AI mask a preset carries: what to find, and the mask's settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdaptiveMask {
    /// What to find (`Shape::Image::what`, e.g. "sky").
    pub what: String,
    #[serde(default)]
    pub invert: bool,
    #[serde(default = "full")]
    pub opacity: f64,
    #[serde(default)]
    pub local: Local,
    #[serde(default)]
    pub range: Range,
}

fn full() -> f64 {
    100.0
}

impl AdaptiveMask {
    /// The mask for a photo, its shape found there.
    pub fn with_shape(&self, shape: Shape) -> Mask {
        Mask {
            shape,
            invert: self.invert,
            opacity: self.opacity,
            hidden: false,
            range: self.range,
            local: self.local,
        }
    }
}

/// The AI masks of `masks` (switched on, found by `what`) as a preset carries them; `keep`
/// says which `what`s can be found again (a click-picked object cannot).
pub fn from_masks(masks: &Masks, keep: impl Fn(&str) -> bool) -> Vec<AdaptiveMask> {
    masks
        .0
        .iter()
        .filter(|m| !m.hidden)
        .filter_map(|m| match &m.shape {
            Shape::Image { what, .. } if keep(what) => Some(AdaptiveMask {
                what: what.clone(),
                invert: m.invert,
                opacity: m.opacity,
                local: m.local,
                range: m.range,
            }),
            _ => None,
        })
        .collect()
}

/// Puts `list` into a preset's JSON (`desktop.adaptiveMasks`); none leaves it as it is.
pub fn write_into_preset(preset: &mut Value, list: &[AdaptiveMask]) {
    if list.is_empty() {
        return;
    }
    let Some(obj) = preset.as_object_mut() else {
        return;
    };
    let desktop = obj
        .entry("desktop")
        .or_insert_with(|| Value::Object(Default::default()));
    if let Some(d) = desktop.as_object_mut() {
        d.insert(
            KEY.to_owned(),
            serde_json::to_value(list).expect("adaptive masks serialise"),
        );
    }
}

/// The AI masks a preset's JSON carries (those that cannot be read are left out).
pub fn from_preset(preset: &Value) -> Vec<AdaptiveMask> {
    let Some(Value::Array(items)) = preset.get("desktop").and_then(|d| d.get(KEY)) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|v| serde_json::from_value::<AdaptiveMask>(v.clone()).ok())
        .filter(|m| !m.what.is_empty() && m.opacity.is_finite())
        .take(crate::masks::MAX_MASKS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn image(what: &str) -> Shape {
        Shape::Image {
            what: what.to_owned(),
            width: 2,
            height: 1,
            data: crate::masks::encode_coverage(&[255, 0]),
        }
    }

    #[test]
    fn ai_masks_go_into_a_preset_and_come_back_with_their_settings() {
        let mut sky = Mask::brush();
        sky.shape = image("sky");
        sky.local.exposure = -0.5;
        sky.opacity = 70.0;
        let mut object = Mask::brush();
        object.shape = image("object");
        let mut hidden = Mask::brush();
        hidden.shape = image("subject");
        hidden.hidden = true;
        let masks = Masks(vec![Mask::radial(), sky, object, hidden]);
        let list = from_masks(&masks, |w| w != "object");
        assert_eq!(
            list.len(),
            1,
            "only the sky: not the radial, object or hidden one"
        );
        let mut preset = json!({"v": 2, "contrast": 10});
        write_into_preset(&mut preset, &list);
        assert_eq!(preset["contrast"], 10);
        let back = from_preset(&preset);
        assert_eq!(back, list);
        let m = back[0].with_shape(image("sky"));
        assert_eq!(m.local.exposure, -0.5);
        assert_eq!(m.opacity, 70.0);
        assert!(from_preset(&json!({"v": 2})).is_empty());
    }
}
