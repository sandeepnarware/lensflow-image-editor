//! Point Color (Lightroom's): up to [`MAX_POINTS`] colours picked in the photo, each with its
//! own hue, saturation and luminance shift, applied to the colours like it. Desktop-only, in
//! `desktop.pointColour`, read and written as [`crate::calibration`] is.
//!
//! The develop shader works on the edited colour (display values, as picked): a colour takes
//! a point's shifts by how close its hue, saturation and brightness are to the point's, each
//! within a reach that grows with the point's Range, and not at all when it is nearly grey.

use serde::{Deserialize, Serialize};

use crate::EditSettings;

pub const MAX_POINTS: usize = 4;
/// The hue shift at ±100, in degrees.
pub const MAX_HUE_SHIFT: f64 = 30.0;

/// One picked colour and what is done to it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Point {
    /// The picked colour: hue in degrees, saturation and brightness 0..1.
    pub hue: f64,
    pub saturation: f64,
    pub brightness: f64,
    /// How wide a family of colours it takes, 0..100.
    pub range: f64,
    /// -100..100 each.
    pub hue_shift: f64,
    pub saturation_shift: f64,
    pub luminance_shift: f64,
}

impl Default for Point {
    fn default() -> Point {
        Point {
            hue: 0.0,
            saturation: 0.5,
            brightness: 0.5,
            range: 50.0,
            hue_shift: 0.0,
            saturation_shift: 0.0,
            luminance_shift: 0.0,
        }
    }
}

fn hsv(rgb: [f64; 3]) -> (f64, f64, f64) {
    let max = rgb[0].max(rgb[1]).max(rgb[2]);
    let min = rgb[0].min(rgb[1]).min(rgb[2]);
    let d = max - min;
    let h = if d <= 1e-9 {
        0.0
    } else if max == rgb[0] {
        60.0 * ((rgb[1] - rgb[2]) / d).rem_euclid(6.0)
    } else if max == rgb[1] {
        60.0 * ((rgb[2] - rgb[0]) / d + 2.0)
    } else {
        60.0 * ((rgb[0] - rgb[1]) / d + 4.0)
    };
    (h, if max <= 0.0 { 0.0 } else { d / max }, max)
}

impl Point {
    /// A point for the colour `rgb` (display values 0..1), with no shifts yet.
    pub fn picked(rgb: [f64; 3]) -> Point {
        let (hue, saturation, _) = hsv(rgb);
        Point {
            hue,
            saturation,
            brightness: 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2],
            ..Point::default()
        }
    }

    fn fixed(self) -> Point {
        let f = |v: f64, lo: f64, hi: f64, d: f64| if v.is_finite() { v.clamp(lo, hi) } else { d };
        Point {
            hue: if self.hue.is_finite() {
                self.hue.rem_euclid(360.0)
            } else {
                0.0
            },
            saturation: f(self.saturation, 0.0, 1.0, 0.5),
            brightness: f(self.brightness, 0.0, 1.0, 0.5),
            range: f(self.range, 0.0, 100.0, 50.0),
            hue_shift: f(self.hue_shift, -100.0, 100.0, 0.0),
            saturation_shift: f(self.saturation_shift, -100.0, 100.0, 0.0),
            luminance_shift: f(self.luminance_shift, -100.0, 100.0, 0.0),
        }
    }

    /// The shader's two rows: (hue, saturation, brightness, range 0..1) and (hue shift in
    /// degrees, saturation shift, luminance shift, both -1..1, unused).
    pub fn uniform(&self) -> [[f32; 4]; 2] {
        [
            [
                self.hue as f32,
                self.saturation as f32,
                self.brightness as f32,
                (self.range / 100.0) as f32,
            ],
            [
                (self.hue_shift / 100.0 * MAX_HUE_SHIFT) as f32,
                (self.saturation_shift / 100.0) as f32,
                (self.luminance_shift / 100.0) as f32,
                0.0,
            ],
        ]
    }
}

/// A photo's points, in the order picked.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Points(pub Vec<Point>);

impl Points {
    /// Read `desktop.pointColour`; what cannot be read (or past [`MAX_POINTS`]) is left out.
    pub fn from_settings(s: &EditSettings) -> Points {
        let Some(serde_json::Value::Array(items)) =
            s.desktop.as_ref().and_then(|d| d.get("pointColour"))
        else {
            return Points::default();
        };
        Points(
            items
                .iter()
                .filter_map(|v| serde_json::from_value::<Point>(v.clone()).ok())
                .map(Point::fixed)
                .take(MAX_POINTS)
                .collect(),
        )
    }

    /// Store into `desktop.pointColour`; none is no key (and an emptied `desktop` is dropped).
    pub fn write_into(&self, s: &mut EditSettings) {
        if self.0.is_empty() {
            if let Some(desktop) = s.desktop.as_mut() {
                desktop.remove("pointColour");
                if desktop.is_empty() {
                    s.desktop = None;
                }
            }
            return;
        }
        s.desktop.get_or_insert_with(Default::default).insert(
            "pointColour".to_string(),
            serde_json::to_value(&self.0).expect("points serialise"),
        );
    }

    /// The points that change anything (a point with no shift is skipped by the shader).
    pub fn active(&self) -> Vec<Point> {
        self.0
            .iter()
            .filter(|p| p.hue_shift != 0.0 || p.saturation_shift != 0.0 || p.luminance_shift != 0.0)
            .copied()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picked_colour_round_trips_and_is_kept_in_range() {
        let p = Point::picked([0.2, 0.4, 0.8]);
        assert!((p.hue - 220.0).abs() < 0.5, "{p:?}");
        assert!((p.saturation - 0.75).abs() < 1e-9);
        let mut s = EditSettings::default_settings();
        Points::default().write_into(&mut s);
        assert!(s.desktop.is_none());
        let mut shifted = p;
        shifted.saturation_shift = 40.0;
        Points(vec![p, shifted]).write_into(&mut s);
        let back = Points::from_settings(&s);
        assert_eq!(back.0, vec![p, shifted]);
        assert_eq!(
            back.active(),
            vec![shifted],
            "a point with no shift does nothing"
        );
        s.desktop.as_mut().unwrap().insert(
            "pointColour".into(),
            serde_json::json!([{"hue": 400, "range": 300}, {}, {}, {}, {}]),
        );
        let read = Points::from_settings(&s);
        assert_eq!(read.0.len(), MAX_POINTS);
        assert_eq!(read.0[0].hue, 40.0);
        assert_eq!(read.0[0].range, 100.0);
    }
}
