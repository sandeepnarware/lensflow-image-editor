//! A preset's or look's Amount (Lightroom's 0–200 %): the photo's settings from before the
//! preset to with it, and past it. Every number the preset changed moves `amount` of the way
//! from its old value to its new one (beyond 1 it keeps going; [`normalise`] clamps it to its
//! range afterwards). Lists of the same length (curve points, colour-mix bands) move element
//! by element. Anything that cannot be part-applied (a switch, a list that changed length, a
//! key one side lacks) takes the preset's value as soon as the amount is above 0.

use serde_json::Value;

use crate::{EditSettings, normalise};

/// The highest Amount (200 %).
pub const MAX_AMOUNT: f32 = 2.0;

fn mix(before: &Value, after: &Value, amount: f64) -> Value {
    match (before, after) {
        (Value::Number(a), Value::Number(b)) => {
            let (a, b) = (a.as_f64().unwrap_or(0.0), b.as_f64().unwrap_or(0.0));
            if a == b {
                return before.clone();
            }
            serde_json::Number::from_f64(a + (b - a) * amount).map_or(Value::Null, Value::Number)
        }
        (Value::Object(a), Value::Object(b)) => {
            let mut out = a.clone();
            for (k, bv) in b {
                let v = match a.get(k) {
                    Some(av) => mix(av, bv, amount),
                    None if amount > 0.0 => bv.clone(),
                    None => continue,
                };
                out.insert(k.clone(), v);
            }
            if amount > 0.0 {
                out.retain(|k, _| b.contains_key(k));
            }
            Value::Object(out)
        }
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => Value::Array(
            a.iter()
                .zip(b)
                .map(|(av, bv)| mix(av, bv, amount))
                .collect(),
        ),
        _ if amount > 0.0 => after.clone(),
        _ => before.clone(),
    }
}

/// `before` moved `amount` (0..[`MAX_AMOUNT`]) of the way to `after`.
pub fn blend(before: &EditSettings, after: &EditSettings, amount: f32) -> EditSettings {
    let amount = f64::from(amount.clamp(0.0, MAX_AMOUNT));
    if amount == 1.0 {
        return after.clone();
    }
    normalise(&mix(&before.to_value(), &after.to_value(), amount))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn with(v: Value) -> EditSettings {
        let mut base = EditSettings::default_settings().to_value();
        if let (Some(m), Some(extra)) = (base.as_object_mut(), v.as_object()) {
            for (k, v) in extra {
                m.insert(k.clone(), v.clone());
            }
        }
        normalise(&base)
    }

    #[test]
    fn numbers_move_part_way_and_past() {
        let before = with(json!({"exposure": 0.0, "contrast": 10}));
        let after = with(json!({"exposure": 1.0, "contrast": 30}));
        assert_eq!(blend(&before, &after, 0.0), before);
        assert_eq!(blend(&before, &after, 1.0), after);
        let half = blend(&before, &after, 0.5).to_value();
        assert_eq!(half["exposure"].as_f64(), Some(0.5));
        assert_eq!(half["contrast"].as_f64(), Some(20.0));
        let double = blend(&before, &after, 2.0).to_value();
        assert_eq!(double["exposure"].as_f64(), Some(2.0));
        assert_eq!(double["contrast"].as_f64(), Some(50.0));
    }

    #[test]
    fn past_the_range_is_clamped() {
        let before = with(json!({"contrast": 0}));
        let after = with(json!({"contrast": 80}));
        let v = blend(&before, &after, 2.0).to_value();
        assert_eq!(v["contrast"].as_f64(), Some(100.0));
    }

    #[test]
    fn switches_take_the_preset_once_above_zero() {
        let before = with(json!({"bw": false}));
        let after = with(json!({"bw": true}));
        assert_eq!(blend(&before, &after, 0.0).to_value()["bw"], json!(false));
        assert_eq!(blend(&before, &after, 0.2).to_value()["bw"], json!(true));
    }

    #[test]
    fn same_length_lists_move_element_by_element() {
        assert_eq!(
            mix(
                &json!([[0, 0], [255, 255]]),
                &json!([[0, 20], [255, 235]]),
                0.5
            ),
            json!([[0, 10.0], [255, 245.0]])
        );
        // A list that changed length cannot be part-applied.
        assert_eq!(
            mix(&json!([1, 2]), &json!([1, 2, 3]), 0.5),
            json!([1, 2, 3])
        );
        assert_eq!(mix(&json!([1, 2]), &json!([1, 2, 3]), 0.0), json!([1, 2]));
    }
}
