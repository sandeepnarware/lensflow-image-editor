//! Colour calibration (Lightroom's Calibration panel): the hue and saturation of the red,
//! green and blue primaries, and a green–magenta tint in the shadows. Desktop-only, in
//! `desktop.calibration`, read and written as [`crate::lens`] is.
//!
//! The primaries are moved as a 3 × 3 matrix on linear RGB ([`Calibration::matrix`]): each
//! primary is turned about the grey axis (hue, up to [`MAX_TURN`] either way: red towards
//! yellow or magenta, green towards cyan or yellow, blue towards magenta or cyan) and moved
//! nearer to or further from grey (saturation). The matrix's columns are the moved primaries,
//! scaled so white stays white; so greys never change, only colours do. The develop shader
//! applies it first, in linear light, then the tint (strongest in the darkest tones).

use serde::{Deserialize, Serialize};

use crate::EditSettings;

/// How far a primary's hue turns at ±100, in degrees.
pub const MAX_TURN: f64 = 30.0;
/// How strongly the shadows tint acts at ±100 (the shader's constant).
pub const TINT: f64 = 0.25;

/// The sliders, each -100..100.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Calibration {
    /// Above 0 moves greens in the shadows towards magenta.
    pub shadows_tint: f64,
    pub red_hue: f64,
    pub red_saturation: f64,
    pub green_hue: f64,
    pub green_saturation: f64,
    pub blue_hue: f64,
    pub blue_saturation: f64,
}

type Mat3 = [[f64; 3]; 3];

/// `v` turned by `angle` (radians) about the grey axis (1, 1, 1).
fn turn(v: [f64; 3], angle: f64) -> [f64; 3] {
    // Rodrigues' rotation about the unit grey axis.
    let k = 1.0 / 3f64.sqrt();
    let (s, c) = angle.sin_cos();
    let dot = (v[0] + v[1] + v[2]) * k;
    let cross = [k * (v[2] - v[1]), k * (v[0] - v[2]), k * (v[1] - v[0])];
    std::array::from_fn(|i| v[i] * c + cross[i] * s + k * dot * (1.0 - c))
}

/// `m x = b` by Cramer's rule.
fn solve(m: Mat3, b: [f64; 3]) -> Option<[f64; 3]> {
    let det = |m: Mat3| {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    let d = det(m);
    (d.abs() > 1e-9).then(|| {
        std::array::from_fn(|i| {
            let mut mi = m;
            for row in 0..3 {
                mi[row][i] = b[row];
            }
            det(mi) / d
        })
    })
}

impl Calibration {
    /// Read `desktop.calibration`; anything missing or unreadable is neutral.
    pub fn from_settings(s: &EditSettings) -> Calibration {
        let c = s
            .desktop
            .as_ref()
            .and_then(|d| d.get("calibration"))
            .and_then(|v| serde_json::from_value::<Calibration>(v.clone()).ok())
            .unwrap_or_default();
        let fix = |v: f64| {
            if v.is_finite() {
                v.clamp(-100.0, 100.0)
            } else {
                0.0
            }
        };
        Calibration {
            shadows_tint: fix(c.shadows_tint),
            red_hue: fix(c.red_hue),
            red_saturation: fix(c.red_saturation),
            green_hue: fix(c.green_hue),
            green_saturation: fix(c.green_saturation),
            blue_hue: fix(c.blue_hue),
            blue_saturation: fix(c.blue_saturation),
        }
    }

    /// Store into `desktop.calibration`; neutral is no key (and an emptied `desktop` is
    /// dropped).
    pub fn write_into(&self, s: &mut EditSettings) {
        if self.is_neutral() {
            if let Some(desktop) = s.desktop.as_mut() {
                desktop.remove("calibration");
                if desktop.is_empty() {
                    s.desktop = None;
                }
            }
            return;
        }
        s.desktop.get_or_insert_with(Default::default).insert(
            "calibration".to_string(),
            serde_json::to_value(self).expect("calibration serialises"),
        );
    }

    /// Nothing is changed.
    pub fn is_neutral(&self) -> bool {
        *self == Calibration::default()
    }

    /// The primaries' matrix on linear RGB, row by row (`out[i] = Σ m[i][j] in[j]`).
    pub fn matrix(&self) -> [[f32; 3]; 3] {
        // Positive hue turns red towards yellow, green towards cyan and blue towards magenta:
        // each towards the next primary round the circle (R → G → B → R).
        let primaries = [
            ([1.0, 0.0, 0.0], self.red_hue, self.red_saturation),
            ([0.0, 1.0, 0.0], self.green_hue, self.green_saturation),
            ([0.0, 0.0, 1.0], self.blue_hue, self.blue_saturation),
        ];
        let cols: [[f64; 3]; 3] = primaries.map(|(p, hue, sat)| {
            let p = turn(p, (hue / 100.0 * MAX_TURN).to_radians());
            // Saturation: the primary's distance from its own grey, scaled.
            let grey = (p[0] + p[1] + p[2]) / 3.0;
            let k = 1.0 + sat / 100.0 * 0.6;
            p.map(|v| grey + (v - grey) * k)
        });
        let m: Mat3 = std::array::from_fn(|i| std::array::from_fn(|j| cols[j][i]));
        // White (the three primaries together) must stay white: each column weighted so.
        let w = solve(m, [1.0, 1.0, 1.0]).unwrap_or([1.0; 3]);
        std::array::from_fn(|i| std::array::from_fn(|j| (m[i][j] * w[j]) as f32))
    }

    /// The shader's shadows tint (scaled; above 0 is magenta).
    pub fn tint(&self) -> f32 {
        (self.shadows_tint / 100.0 * TINT) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(m: &[[f32; 3]; 3], c: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|i| (0..3).map(|j| m[i][j] * c[j]).sum())
    }

    #[test]
    fn neutral_is_the_identity_and_stores_nothing() {
        let m = Calibration::default().matrix();
        for (i, row) in m.iter().enumerate() {
            for (j, v) in row.iter().enumerate() {
                assert!((v - f32::from(u8::from(i == j))).abs() < 1e-6);
            }
        }
        let mut s = EditSettings::default_settings();
        Calibration::default().write_into(&mut s);
        assert_eq!(s.desktop, None);
    }

    #[test]
    fn greys_never_move_whatever_the_sliders() {
        let c = Calibration {
            red_hue: 80.0,
            red_saturation: -60.0,
            green_hue: -40.0,
            green_saturation: 100.0,
            blue_hue: 30.0,
            blue_saturation: 50.0,
            shadows_tint: 0.0,
        };
        let m = c.matrix();
        for g in [0.05f32, 0.5, 1.0, 3.0] {
            let out = apply(&m, [g, g, g]);
            for v in out {
                assert!((v - g).abs() < 1e-4 * g.max(1.0), "{g}: {out:?}");
            }
        }
    }

    #[test]
    fn red_hue_turns_red_towards_yellow_and_saturation_deepens_it() {
        let warm = Calibration {
            red_hue: 100.0,
            ..Calibration::default()
        };
        let out = apply(&warm.matrix(), [0.8, 0.2, 0.2]);
        assert!(out[1] > out[2] + 0.02, "towards yellow: {out:?}");
        let rich = Calibration {
            red_saturation: 100.0,
            ..Calibration::default()
        };
        let plain = [0.8f32, 0.2, 0.2];
        let out = apply(&rich.matrix(), plain);
        assert!(
            out[0] - out[1] > plain[0] - plain[1],
            "more saturated: {out:?}"
        );
        // A pure green is barely touched by the red sliders.
        let g = apply(&rich.matrix(), [0.2, 0.8, 0.2]);
        assert!((g[1] - 0.8).abs() < 0.1, "{g:?}");
    }

    #[test]
    fn it_round_trips_through_the_settings() {
        let c = Calibration {
            shadows_tint: 12.0,
            blue_saturation: -30.0,
            ..Calibration::default()
        };
        let mut s = EditSettings::default_settings();
        c.write_into(&mut s);
        assert_eq!(Calibration::from_settings(&s), c);
        assert!((c.tint() - 0.03).abs() < 1e-6);
    }
}
