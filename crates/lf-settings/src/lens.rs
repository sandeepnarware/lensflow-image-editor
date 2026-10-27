//! Lens corrections: distortion, the lens's own vignetting, lateral chromatic aberration and
//! perspective (vertical and horizontal keystone). Desktop-only, in `desktop.lens`, read and
//! written as [`crate::masks`] and [`crate::DevelopSettings`] are.
//!
//! They happen first, before the crop and turn: the photo is corrected, then framed. So the
//! develop shader maps each output pixel through the geometry to a *corrected* position and
//! then, with [`Lens::to_source`], to where it is in the photo as shot. This file and the
//! shader's `lens_to_source` are the same formula; the spot tool uses [`Lens::to_source`] and
//! [`Lens::to_corrected`] to place spots (which live in the photo as shot) under the pointer.
//!
//! Distances are measured in a square space centred on the photo (x × aspect) and scaled so
//! a corner is at radius 1, so a correction looks the same on any shape of photo.

use serde::{Deserialize, Serialize};

use crate::EditSettings;

/// The sliders, each -100..100 (vignetting 0..100 lightens; below 0 darkens the corners).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Lens {
    /// Above 0 straightens barrel distortion (lines bowing out); below 0, pincushion.
    pub distortion: f64,
    /// Above 0 lightens dark corners.
    pub vignetting: f64,
    /// Red/cyan fringes: moves the red channel in or out.
    pub ca_red: f64,
    /// Blue/yellow fringes: moves the blue channel.
    pub ca_blue: f64,
    /// Above 0 straightens verticals that lean in at the top (a building shot from below).
    pub vertical: f64,
    /// Above 0 straightens horizontals that lean in at the right.
    pub horizontal: f64,
}

/// How strongly each slider at 100 acts (the shader's constants).
pub const DISTORTION: f64 = 0.3;
pub const VIGNETTING: f64 = 1.0;
pub const CA: f64 = 0.004;
pub const KEYSTONE: f64 = 0.6;

impl Lens {
    /// Read `desktop.lens`; anything missing or unreadable is no correction.
    pub fn from_settings(s: &EditSettings) -> Lens {
        let lens = s
            .desktop
            .as_ref()
            .and_then(|d| d.get("lens"))
            .and_then(|v| serde_json::from_value::<Lens>(v.clone()).ok())
            .unwrap_or_default();
        let fix = |v: f64| {
            if v.is_finite() {
                v.clamp(-100.0, 100.0)
            } else {
                0.0
            }
        };
        Lens {
            distortion: fix(lens.distortion),
            vignetting: fix(lens.vignetting),
            ca_red: fix(lens.ca_red),
            ca_blue: fix(lens.ca_blue),
            vertical: fix(lens.vertical),
            horizontal: fix(lens.horizontal),
        }
    }

    /// Store into `desktop.lens`; none is no key (and an emptied `desktop` is dropped).
    pub fn write_into(&self, s: &mut EditSettings) {
        if self.is_none() {
            if let Some(desktop) = s.desktop.as_mut() {
                desktop.remove("lens");
                if desktop.is_empty() {
                    s.desktop = None;
                }
            }
            return;
        }
        s.desktop.get_or_insert_with(Default::default).insert(
            "lens".to_string(),
            serde_json::to_value(self).expect("lens serialises"),
        );
    }

    /// Nothing is corrected.
    pub fn is_none(&self) -> bool {
        *self == Lens::default()
    }

    /// Only the geometry moves (distortion or keystone): the spot tool must map through it.
    pub fn moves_pixels(&self) -> bool {
        self.distortion != 0.0 || self.vertical != 0.0 || self.horizontal != 0.0
    }

    /// The corrected position `(u, v)` (photo fractions, after correction) to where it is
    /// in the photo as shot, for a photo `aspect` wide per unit of height.
    pub fn to_source(&self, (u, v): (f64, f64), aspect: f64) -> (f64, f64) {
        // Keystone first (it was applied last): each row (column) is scaled about the centre
        // by how far down (across) it is.
        let kv = self.vertical / 100.0 * KEYSTONE;
        let kh = self.horizontal / 100.0 * KEYSTONE;
        let (mut x, mut y) = (u - 0.5, v - 0.5);
        let sx = 1.0 + kv * y;
        let sy = 1.0 - kh * x;
        x *= sx;
        y *= sy;
        // Then the radial distortion, in the square space with the corner at radius 1.
        let a = -self.distortion / 100.0 * DISTORTION;
        let corner2 = (aspect * aspect + 1.0) / 4.0;
        let r2 = ((x * aspect).powi(2) + y * y) / corner2;
        let f = 1.0 + a * r2;
        (0.5 + x * f, 0.5 + y * f)
    }

    /// The inverse of [`Lens::to_source`]: where a point of the photo as shot lands after
    /// correction (a few Newton steps; exact for no correction).
    pub fn to_corrected(&self, p: (f64, f64), aspect: f64) -> (f64, f64) {
        if !self.moves_pixels() {
            return p;
        }
        let mut q = p;
        for _ in 0..12 {
            let f = self.to_source(q, aspect);
            let (ex, ey) = (f.0 - p.0, f.1 - p.1);
            if ex.abs() < 1e-12 && ey.abs() < 1e-12 {
                break;
            }
            // The Jacobian by small differences.
            let h = 1e-6;
            let fx = self.to_source((q.0 + h, q.1), aspect);
            let fy = self.to_source((q.0, q.1 + h), aspect);
            let (a, b) = ((fx.0 - f.0) / h, (fy.0 - f.0) / h);
            let (c, d) = ((fx.1 - f.1) / h, (fy.1 - f.1) / h);
            let det = a * d - b * c;
            if det.abs() < 1e-12 {
                break;
            }
            q = (
                q.0 - (d * ex - b * ey) / det,
                q.1 - (-c * ex + a * ey) / det,
            );
        }
        q
    }

    /// The lens vignetting gain at a point of the photo as shot (linear light).
    pub fn vignette_gain(&self, (u, v): (f64, f64), aspect: f64) -> f64 {
        let corner2 = (aspect * aspect + 1.0) / 4.0;
        let r2 = (((u - 0.5) * aspect).powi(2) + (v - 0.5).powi(2)) / corner2;
        1.0 + self.vignetting / 100.0 * VIGNETTING * r2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_leaves_every_point_and_stores_nothing() {
        let lens = Lens::default();
        for p in [(0.0, 0.0), (0.3, 0.8), (1.0, 1.0)] {
            assert_eq!(lens.to_source(p, 1.5), p);
            assert_eq!(lens.to_corrected(p, 1.5), p);
        }
        let mut s = EditSettings::default_settings();
        lens.write_into(&mut s);
        assert!(s.desktop.is_none());
        assert_eq!(Lens::from_settings(&s), lens);
    }

    #[test]
    fn round_trips_through_desktop_and_clamps() {
        let mut s = EditSettings::default_settings();
        let lens = Lens {
            distortion: 40.0,
            vertical: -25.0,
            ..Lens::default()
        };
        lens.write_into(&mut s);
        assert_eq!(Lens::from_settings(&s), lens);
        s.desktop.as_mut().unwrap().insert(
            "lens".into(),
            serde_json::json!({"distortion": 500, "caRed": "x"}),
        );
        assert_eq!(Lens::from_settings(&s), Lens::default(), "unreadable: none");
        s.desktop
            .as_mut()
            .unwrap()
            .insert("lens".into(), serde_json::json!({"distortion": 500}));
        assert_eq!(Lens::from_settings(&s).distortion, 100.0);
    }

    #[test]
    fn barrel_correction_pulls_the_edges_in_from_closer_to_the_centre() {
        let lens = Lens {
            distortion: 50.0,
            ..Lens::default()
        };
        // The centre stays; a corner samples from nearer the centre (so it is stretched out).
        assert_eq!(lens.to_source((0.5, 0.5), 1.5), (0.5, 0.5));
        let (u, v) = lens.to_source((1.0, 1.0), 1.5);
        assert!(u < 1.0 && v < 1.0 && u > 0.5 && v > 0.5, "{u} {v}");
        assert!((u - (0.5 + 0.5 * (1.0 - 0.15))).abs() < 1e-9);
    }

    #[test]
    fn vertical_keystone_widens_the_top() {
        let lens = Lens {
            vertical: 50.0,
            ..Lens::default()
        };
        // At the top the right edge samples from further in; at the bottom from further out.
        assert!(lens.to_source((1.0, 0.0), 1.0).0 < 1.0);
        assert!(lens.to_source((1.0, 1.0), 1.0).0 > 1.0);
        assert_eq!(
            lens.to_source((0.5, 0.0), 1.0).0,
            0.5,
            "the middle stays upright"
        );
    }

    #[test]
    fn to_corrected_inverts_to_source() {
        let lens = Lens {
            distortion: -60.0,
            vertical: 35.0,
            horizontal: -20.0,
            ..Lens::default()
        };
        for p in [(0.1, 0.2), (0.5, 0.5), (0.9, 0.7), (0.3, 0.95)] {
            let back = lens.to_source(lens.to_corrected(p, 1.5), 1.5);
            assert!(
                (back.0 - p.0).abs() < 1e-9 && (back.1 - p.1).abs() < 1e-9,
                "{p:?}"
            );
        }
    }

    #[test]
    fn vignetting_lifts_the_corners_only() {
        let lens = Lens {
            vignetting: 50.0,
            ..Lens::default()
        };
        assert_eq!(lens.vignette_gain((0.5, 0.5), 1.5), 1.0);
        assert!((lens.vignette_gain((0.0, 0.0), 1.5) - 1.5).abs() < 1e-9);
    }
}
