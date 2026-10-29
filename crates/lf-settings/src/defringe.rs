//! Defringe (Lightroom's): purple and green fringes, the axial colour a lens leaves along
//! high-contrast edges, taken out. Desktop-only, in `desktop.defringe`, read and written as
//! [`crate::calibration`] is.
//!
//! The develop shader greys a pixel's colour (towards its own brightness) by the amount when
//! it is near a strong edge (a brightness step of 0.05 or more within four pixels) and its hue
//! is within [`WIDTH`] degrees of the purple or the green hue chosen; the photo elsewhere, and
//! purple or green away from edges, is left alone.

use serde::{Deserialize, Serialize};

use crate::EditSettings;

/// The amounts' top (as Lightroom's 0–20).
pub const MAX_AMOUNT: f64 = 20.0;
/// The hues' ranges and defaults, in degrees.
pub const PURPLE_HUES: (f64, f64) = (240.0, 330.0);
pub const GREEN_HUES: (f64, f64) = (75.0, 165.0);
pub const PURPLE_HUE: f64 = 290.0;
pub const GREEN_HUE: f64 = 115.0;
/// How far from the chosen hue a fringe's colour may be, in degrees.
pub const WIDTH: f64 = 35.0;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Defringe {
    pub purple_amount: f64,
    pub purple_hue: f64,
    pub green_amount: f64,
    pub green_hue: f64,
}

impl Default for Defringe {
    fn default() -> Defringe {
        Defringe {
            purple_amount: 0.0,
            purple_hue: PURPLE_HUE,
            green_amount: 0.0,
            green_hue: GREEN_HUE,
        }
    }
}

fn fix(v: f64, (lo, hi): (f64, f64), default: f64) -> f64 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        default
    }
}

impl Defringe {
    /// Read `desktop.defringe`; anything missing or unreadable is off.
    pub fn from_settings(s: &EditSettings) -> Defringe {
        let d = s
            .desktop
            .as_ref()
            .and_then(|d| d.get("defringe"))
            .and_then(|v| serde_json::from_value::<Defringe>(v.clone()).ok())
            .unwrap_or_default();
        Defringe {
            purple_amount: fix(d.purple_amount, (0.0, MAX_AMOUNT), 0.0),
            purple_hue: fix(d.purple_hue, PURPLE_HUES, PURPLE_HUE),
            green_amount: fix(d.green_amount, (0.0, MAX_AMOUNT), 0.0),
            green_hue: fix(d.green_hue, GREEN_HUES, GREEN_HUE),
        }
    }

    /// Store into `desktop.defringe`; off (both amounts 0) is no key (and an emptied
    /// `desktop` is dropped).
    pub fn write_into(&self, s: &mut EditSettings) {
        if self.is_off() {
            if let Some(desktop) = s.desktop.as_mut() {
                desktop.remove("defringe");
                if desktop.is_empty() {
                    s.desktop = None;
                }
            }
            return;
        }
        s.desktop.get_or_insert_with(Default::default).insert(
            "defringe".to_string(),
            serde_json::to_value(self).expect("defringe serialises"),
        );
    }

    /// Nothing is taken out.
    pub fn is_off(&self) -> bool {
        self.purple_amount <= 0.0 && self.green_amount <= 0.0
    }

    /// The shader's `fringe`: purple amount (0..1), purple hue, green amount, green hue.
    pub fn uniform(&self) -> [f32; 4] {
        [
            (self.purple_amount / MAX_AMOUNT) as f32,
            self.purple_hue as f32,
            (self.green_amount / MAX_AMOUNT) as f32,
            self.green_hue as f32,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_is_no_key_and_values_are_kept_in_range() {
        let mut s = EditSettings::default_settings();
        Defringe::default().write_into(&mut s);
        assert!(s.desktop.is_none());
        let d = Defringe {
            purple_amount: 8.0,
            ..Defringe::default()
        };
        d.write_into(&mut s);
        assert_eq!(Defringe::from_settings(&s), d);
        s.desktop.as_mut().unwrap().insert(
            "defringe".into(),
            serde_json::json!({"purpleAmount": 99, "purpleHue": 10}),
        );
        let read = Defringe::from_settings(&s);
        assert_eq!(read.purple_amount, MAX_AMOUNT);
        assert_eq!(read.purple_hue, PURPLE_HUES.0);
        assert_eq!(read.green_hue, GREEN_HUE, "missing: the default");
        assert_eq!(read.uniform()[0], 1.0);
    }
}
