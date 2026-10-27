//! Local adjustments ("masks"): areas of the photo, each with its own exposure, contrast,
//! tone, white balance, saturation and clarity, as Lightroom's masking panel has them.
//!
//! They are desktop-only and live in `desktop.masks` (as `desktop.develop` does), so the web
//! editor keeps them in a sidecar it rewrites but does not draw them. Read them with
//! [`Masks::from_settings`] (anything unreadable is dropped, never an error) and store them
//! with [`Masks::write_into`] (no masks, no key).
//!
//! Shapes are in source-photo fractions (the space the spots use), so they stay on the same
//! part of the photo when it is cropped, turned or flipped. Lengths that must be the same in
//! both directions (a radial's radii, nothing else) are fractions of the photo's *height*;
//! the shader multiplies x by the photo's aspect to make its space square.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::EditSettings;

/// The most masks a photo can have (the shader's array).
pub const MAX_MASKS: usize = 8;

/// The area a mask covers.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Shape {
    /// A graduated filter: full effect behind the start line (`x0`, `y0`), none past the end
    /// line (`x1`, `y1`), fading between them. Points in photo fractions.
    Linear { x0: f64, y0: f64, x1: f64, y1: f64 },
    /// An ellipse around (`cx`, `cy`) (photo fractions) with radii `rx`, `ry` (fractions of
    /// the photo's height), turned by `angle` degrees. Full effect inside; `feather` sets how
    /// far in from the edge the fade starts.
    Radial {
        cx: f64,
        cy: f64,
        rx: f64,
        ry: f64,
        angle: f64,
        feather: f64,
    },
}

/// What a mask does where it applies: the global sliders' units (exposure in stops, the rest
/// -100..100), added to the global values there.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Local {
    pub exposure: f64,
    pub contrast: f64,
    pub highlights: f64,
    pub shadows: f64,
    pub whites: f64,
    pub blacks: f64,
    pub temperature: f64,
    pub tint: f64,
    pub saturation: f64,
    pub clarity: f64,
}

impl Local {
    /// The sliders, in panel order: label, value, and whether it is the exposure (stops).
    pub fn sliders(&mut self) -> [(&'static str, &mut f64); 10] {
        [
            ("Exposure", &mut self.exposure),
            ("Contrast", &mut self.contrast),
            ("Highlights", &mut self.highlights),
            ("Shadows", &mut self.shadows),
            ("Whites", &mut self.whites),
            ("Blacks", &mut self.blacks),
            ("Temperature", &mut self.temperature),
            ("Tint", &mut self.tint),
            ("Saturation", &mut self.saturation),
            ("Clarity", &mut self.clarity),
        ]
    }

    fn clamped(mut self) -> Local {
        let fix = |v: f64, lo: f64, hi: f64| if v.is_finite() { v.clamp(lo, hi) } else { 0.0 };
        self.exposure = fix(self.exposure, -4.0, 4.0);
        for (_, v) in self.sliders().into_iter().skip(1) {
            *v = fix(*v, -100.0, 100.0);
        }
        self
    }
}

/// One local adjustment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mask {
    pub shape: Shape,
    /// Everywhere except the shape instead.
    #[serde(default)]
    pub invert: bool,
    #[serde(default)]
    pub local: Local,
}

impl Mask {
    /// A new graduated filter from the top down to the middle.
    pub fn linear() -> Mask {
        Mask {
            shape: Shape::Linear {
                x0: 0.5,
                y0: 0.15,
                x1: 0.5,
                y1: 0.5,
            },
            invert: false,
            local: Local::default(),
        }
    }

    /// A new ellipse in the middle, a third of the photo's height across.
    pub fn radial() -> Mask {
        Mask {
            shape: Shape::Radial {
                cx: 0.5,
                cy: 0.5,
                rx: 0.3,
                ry: 0.2,
                angle: 0.0,
                feather: 50.0,
            },
            invert: false,
            local: Local::default(),
        }
    }

    /// What the mask is called in the panel.
    pub fn kind_name(&self) -> &'static str {
        match self.shape {
            Shape::Linear { .. } => "Linear gradient",
            Shape::Radial { .. } => "Radial gradient",
        }
    }

    /// The same mask with every number made safe (finite, in range): points may sit a little
    /// outside the photo, radii are positive, angles wrap.
    fn clamped(mut self) -> Option<Mask> {
        let pos = |v: f64| {
            if v.is_finite() {
                v.clamp(-1.0, 2.0)
            } else {
                0.5
            }
        };
        self.shape = match self.shape {
            Shape::Linear { x0, y0, x1, y1 } => {
                let (x0, y0, x1, y1) = (pos(x0), pos(y0), pos(x1), pos(y1));
                // Two lines on top of each other say nothing about a direction.
                if (x1 - x0).hypot(y1 - y0) < 1e-6 {
                    return None;
                }
                Shape::Linear { x0, y0, x1, y1 }
            }
            Shape::Radial {
                cx,
                cy,
                rx,
                ry,
                angle,
                feather,
            } => {
                let radius = |v: f64| {
                    if v.is_finite() {
                        v.clamp(0.002, 4.0)
                    } else {
                        0.2
                    }
                };
                Shape::Radial {
                    cx: pos(cx),
                    cy: pos(cy),
                    rx: radius(rx),
                    ry: radius(ry),
                    angle: if angle.is_finite() {
                        angle.rem_euclid(360.0)
                    } else {
                        0.0
                    },
                    feather: if feather.is_finite() {
                        feather.clamp(0.0, 100.0)
                    } else {
                        50.0
                    },
                }
            }
        };
        self.local = self.local.clamped();
        Some(self)
    }
}

/// A photo's masks, in the order they were added.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Masks(pub Vec<Mask>);

impl Masks {
    /// Read `desktop.masks`; a mask that cannot be read (or past [`MAX_MASKS`]) is left out.
    pub fn from_settings(s: &EditSettings) -> Masks {
        let Some(Value::Array(items)) = s.desktop.as_ref().and_then(|d| d.get("masks")) else {
            return Masks::default();
        };
        Masks(
            items
                .iter()
                .filter_map(|v| serde_json::from_value::<Mask>(v.clone()).ok())
                .filter_map(Mask::clamped)
                .take(MAX_MASKS)
                .collect(),
        )
    }

    /// Store into `desktop.masks`; none is stored as no key, and an emptied `desktop` is
    /// dropped.
    pub fn write_into(&self, s: &mut EditSettings) {
        if self.0.is_empty() {
            if let Some(desktop) = s.desktop.as_mut() {
                desktop.remove("masks");
                if desktop.is_empty() {
                    s.desktop = None;
                }
            }
            return;
        }
        let value = serde_json::to_value(&self.0).expect("masks serialise");
        s.desktop
            .get_or_insert_with(Default::default)
            .insert("masks".to_string(), value);
    }
}

/// How much `mask` applies at `(u, v)` (photo fractions) of a photo `aspect` wide per unit of
/// height: 0..=1. The shader's `mask_weight`, for tests and for the panel's overlay.
pub fn weight(mask: &Mask, (u, v): (f64, f64), aspect: f64) -> f64 {
    let p = (u * aspect, v);
    let w = match mask.shape {
        Shape::Linear { x0, y0, x1, y1 } => {
            let a = (x0 * aspect, y0);
            let d = (x1 * aspect - a.0, y1 - a.1);
            let t = ((p.0 - a.0) * d.0 + (p.1 - a.1) * d.1) / (d.0 * d.0 + d.1 * d.1);
            1.0 - smoothstep(0.0, 1.0, t)
        }
        Shape::Radial {
            cx,
            cy,
            rx,
            ry,
            angle,
            feather,
        } => {
            let (s, c) = angle.to_radians().sin_cos();
            let q = (p.0 - cx * aspect, p.1 - cy);
            // Into the ellipse's own axes.
            let q = (q.0 * c + q.1 * s, -q.0 * s + q.1 * c);
            let r = (q.0 / rx).hypot(q.1 / ry);
            1.0 - smoothstep(1.0 - feather / 100.0, 1.0 + 1e-4, r)
        }
    };
    if mask.invert { 1.0 - w } else { w }
}

fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_round_trip_through_desktop_and_none_leaves_no_key() {
        let mut s = EditSettings::default_settings();
        assert_eq!(Masks::from_settings(&s), Masks::default());
        let mut radial = Mask::radial();
        radial.local.exposure = 0.7;
        radial.invert = true;
        let masks = Masks(vec![Mask::linear(), radial]);
        masks.write_into(&mut s);
        assert_eq!(Masks::from_settings(&s), masks);
        // Survives the trip through JSON and normalise, as a sidecar would carry it.
        let json = serde_json::to_string(&s).unwrap();
        let back = crate::normalise(&serde_json::from_str(&json).unwrap());
        assert_eq!(Masks::from_settings(&back), masks);
        // Other desktop keys stay; no masks, no key; nothing left, no desktop.
        Masks::default().write_into(&mut s);
        assert!(s.desktop.is_none());
    }

    #[test]
    fn unreadable_and_out_of_range_masks_are_made_safe() {
        let mut s = EditSettings::default_settings();
        let mut desktop = serde_json::Map::new();
        desktop.insert(
            "masks".into(),
            serde_json::json!([
                {"shape": {"kind": "radial", "cx": 0.5, "cy": 0.5, "rx": -3, "ry": 1e9,
                           "angle": 400, "feather": 250},
                 "local": {"exposure": 9, "contrast": -500}},
                {"shape": {"kind": "star"}},
                "junk",
                {"shape": {"kind": "linear", "x0": 0.2, "y0": 0.2, "x1": 0.2, "y1": 0.2}},
                {"shape": {"kind": "linear", "x0": 0, "y0": 0, "x1": 0, "y1": 1}}
            ]),
        );
        s.desktop = Some(desktop);
        let m = Masks::from_settings(&s);
        assert_eq!(
            m.0.len(),
            2,
            "the star, the junk and the zero-length line are dropped"
        );
        let Shape::Radial {
            rx,
            ry,
            angle,
            feather,
            ..
        } = m.0[0].shape
        else {
            panic!()
        };
        assert_eq!((rx, ry, angle, feather), (0.002, 4.0, 40.0, 100.0));
        assert_eq!(
            (m.0[0].local.exposure, m.0[0].local.contrast),
            (4.0, -100.0)
        );
        assert!(matches!(m.0[1].shape, Shape::Linear { .. }));
    }

    #[test]
    fn no_more_than_eight() {
        let mut s = EditSettings::default_settings();
        Masks(vec![Mask::radial(); 12]).write_into(&mut s);
        assert_eq!(Masks::from_settings(&s).0.len(), MAX_MASKS);
    }

    #[test]
    fn a_linear_mask_fades_from_its_start_to_its_end() {
        let m = Mask {
            shape: Shape::Linear {
                x0: 0.5,
                y0: 0.2,
                x1: 0.5,
                y1: 0.6,
            },
            invert: false,
            local: Local::default(),
        };
        assert_eq!(weight(&m, (0.3, 0.1), 1.5), 1.0, "behind the start");
        assert_eq!(weight(&m, (0.9, 0.7), 1.5), 0.0, "past the end");
        let mid = weight(&m, (0.5, 0.4), 1.5);
        assert!((mid - 0.5).abs() < 1e-9, "{mid}");
        let inverted = Mask { invert: true, ..m };
        assert_eq!(weight(&inverted, (0.3, 0.1), 1.5), 0.0);
    }

    #[test]
    fn a_radial_mask_is_round_on_a_wide_photo_and_feathers_at_its_edge() {
        // A circle 0.2 of the height across on a 3:2 photo.
        let m = Mask {
            shape: Shape::Radial {
                cx: 0.5,
                cy: 0.5,
                rx: 0.2,
                ry: 0.2,
                angle: 0.0,
                feather: 0.0,
            },
            invert: false,
            local: Local::default(),
        };
        let aspect = 1.5;
        assert_eq!(weight(&m, (0.5, 0.5), aspect), 1.0);
        // 0.19 of the height to the right is 0.19 / 1.5 of the width: inside.
        assert_eq!(weight(&m, (0.5 + 0.19 / aspect, 0.5), aspect), 1.0);
        assert_eq!(weight(&m, (0.5 + 0.21 / aspect, 0.5), aspect), 0.0);
        assert_eq!(weight(&m, (0.5, 0.71), aspect), 0.0);
        let soft = Mask {
            shape: Shape::Radial {
                cx: 0.5,
                cy: 0.5,
                rx: 0.2,
                ry: 0.2,
                angle: 0.0,
                feather: 50.0,
            },
            ..m
        };
        let edge = weight(&soft, (0.5, 0.5 + 0.15), aspect);
        assert!(edge > 0.0 && edge < 1.0, "{edge}");
        assert_eq!(
            weight(&soft, (0.5, 0.5 + 0.09), aspect),
            1.0,
            "the core is full"
        );
    }

    #[test]
    fn a_turned_ellipse_follows_its_angle() {
        let m = Mask {
            shape: Shape::Radial {
                cx: 0.5,
                cy: 0.5,
                rx: 0.3,
                ry: 0.05,
                angle: 90.0,
                feather: 0.0,
            },
            invert: false,
            local: Local::default(),
        };
        // Turned a quarter: long up and down, narrow across.
        assert_eq!(weight(&m, (0.5, 0.75), 1.0), 1.0);
        assert_eq!(weight(&m, (0.6, 0.5), 1.0), 0.0);
    }
}
