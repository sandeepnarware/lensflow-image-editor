//! Desktop only: healing (or cloning) along a brushed stroke, for what a round spot cannot
//! cover well (a stray hair, a wire, a scratch). Kept in `desktop.heal`, which the web
//! editor keeps but does not draw, so the web's own `spots` are untouched.
//!
//! A stroke is a line through `points` (photo fractions) `r` wide each side (a fraction of the
//! photo's width, as a spot's radius), copied from the same line moved by `(dx, dy)`. The
//! renderer heals it as a chain of overlapping round dabs ([`dabs`]), each a spot sharing the
//! stroke's offset and knowing the stroke's direction, so the tone match looks at the
//! stroke's sides rather than along the blemish itself.

use crate::{EditSettings, SpotMode};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The most strokes a photo keeps, and the most points one keeps.
pub const MAX_STROKES: usize = 64;
pub const MAX_POINTS: usize = 2000;
/// The brush's radius limits (fractions of the photo's width).
pub const MIN_RADIUS: f64 = 0.002;
pub const MAX_RADIUS: f64 = 0.1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HealStroke {
    pub points: Vec<[f64; 2]>,
    pub r: f64,
    pub dx: f64,
    pub dy: f64,
    #[serde(default = "heal")]
    pub mode: SpotMode,
}

fn heal() -> SpotMode {
    SpotMode::Heal
}

impl HealStroke {
    /// The same stroke with safe numbers, or none when nothing is left of it.
    fn clamped(mut self) -> Option<HealStroke> {
        let ok = |v: f64| v.is_finite();
        self.points.retain(|p| ok(p[0]) && ok(p[1]));
        self.points.truncate(MAX_POINTS);
        for p in &mut self.points {
            *p = p.map(|v| v.clamp(-0.5, 1.5));
        }
        if self.points.is_empty() || !ok(self.r) || !ok(self.dx) || !ok(self.dy) {
            return None;
        }
        self.r = self.r.clamp(MIN_RADIUS, MAX_RADIUS);
        self.dx = self.dx.clamp(-1.0, 1.0);
        self.dy = self.dy.clamp(-1.0, 1.0);
        Some(self)
    }

    /// The source offset a new stroke takes: to the side of its overall direction, three
    /// radii away (in the width's units, so the same distance across and down). `aspect`:
    /// the photo's width per unit of height.
    pub fn default_offset(points: &[[f64; 2]], r: f64, aspect: f64) -> (f64, f64) {
        let (a, b) = match (points.first(), points.last()) {
            (Some(a), Some(b)) => (*a, *b),
            _ => return (3.0 * r, 0.0),
        };
        // Direction in width units (y scaled to the width).
        let (ux, uy) = (b[0] - a[0], (b[1] - a[1]) / aspect);
        let len = ux.hypot(uy);
        let (nx, ny) = if len < 1e-9 {
            (1.0, 0.0)
        } else {
            (-uy / len, ux / len)
        };
        (nx * 3.0 * r, ny * 3.0 * r * aspect)
    }
}

/// One round dab of a stroke: where it heals, where it copies from, its radius (fraction of
/// the width), its mode, and the stroke's direction there (radians, in pixels: y down).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dab {
    pub x: f64,
    pub y: f64,
    pub sx: f64,
    pub sy: f64,
    pub r: f64,
    pub mode: SpotMode,
    pub dir: f64,
}

/// The dabs that heal `stroke` on a photo `aspect` wide per unit of height: centres every
/// 0.4 radius along the line, so they overlap into an even band.
pub fn dabs(stroke: &HealStroke, aspect: f64) -> Vec<Dab> {
    let step = stroke.r * 0.4;
    let mut out = Vec::new();
    let dab = |p: [f64; 2], dir: f64| Dab {
        x: p[0],
        y: p[1],
        sx: p[0] + stroke.dx,
        sy: p[1] + stroke.dy,
        r: stroke.r,
        mode: stroke.mode,
        dir,
    };
    let pts = &stroke.points;
    if pts.len() == 1 {
        out.push(dab(pts[0], 0.0));
        return out;
    }
    let mut carry = 0.0;
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let (ux, uy) = (b[0] - a[0], (b[1] - a[1]) / aspect);
        let len = ux.hypot(uy);
        if len < 1e-12 {
            continue;
        }
        let dir = uy.atan2(ux);
        let mut t = carry;
        while t <= len {
            let k = t / len;
            out.push(dab(
                [a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k],
                dir,
            ));
            t += step;
        }
        carry = t - len;
    }
    // The end itself, unless a dab already sits on it.
    if let (Some(last), Some(end)) = (out.last().copied(), pts.last()) {
        let gap = (end[0] - last.x).hypot((end[1] - last.y) / aspect);
        if gap > step * 0.25 {
            out.push(dab(*end, last.dir));
        }
    }
    if out.is_empty() {
        out.push(dab(pts[0], 0.0));
    }
    out
}

/// A photo's healing strokes, in the order made.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Heals(pub Vec<HealStroke>);

impl Heals {
    /// Read `desktop.heal`; a stroke that cannot be read is left out.
    pub fn from_settings(s: &EditSettings) -> Heals {
        let Some(Value::Array(items)) = s.desktop.as_ref().and_then(|d| d.get("heal")) else {
            return Heals::default();
        };
        Heals(
            items
                .iter()
                .filter_map(|v| serde_json::from_value::<HealStroke>(v.clone()).ok())
                .filter_map(HealStroke::clamped)
                .take(MAX_STROKES)
                .collect(),
        )
    }

    /// Store into `desktop.heal`; none is stored as no key, and an emptied `desktop` is
    /// dropped.
    pub fn write_into(&self, s: &mut EditSettings) {
        if self.0.is_empty() {
            if let Some(desktop) = s.desktop.as_mut() {
                desktop.remove("heal");
                if desktop.is_empty() {
                    s.desktop = None;
                }
            }
            return;
        }
        let value = serde_json::to_value(&self.0).expect("strokes serialise");
        s.desktop
            .get_or_insert_with(Default::default)
            .insert("heal".to_string(), value);
    }

    /// Every stroke's dabs, in order.
    pub fn dabs(&self, aspect: f64) -> Vec<Dab> {
        self.0.iter().flat_map(|s| dabs(s, aspect)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(points: Vec<[f64; 2]>) -> HealStroke {
        HealStroke {
            points,
            r: 0.01,
            dx: 0.03,
            dy: 0.0,
            mode: SpotMode::Heal,
        }
    }

    #[test]
    fn a_stroke_becomes_evenly_spaced_dabs_with_its_offset_and_direction() {
        let s = stroke(vec![[0.1, 0.5], [0.2, 0.5]]);
        let d = dabs(&s, 1.5);
        // 0.1 long at a 0.004 step: 26 dabs (both ends).
        assert_eq!(d.len(), 26);
        assert!((d[1].x - d[0].x - 0.004).abs() < 1e-9);
        assert!((d[0].sx - d[0].x - 0.03).abs() < 1e-12);
        assert!(d.iter().all(|d| d.dir.abs() < 1e-9));
        // Straight down: the direction is a quarter turn (y down in pixels).
        let down = dabs(&stroke(vec![[0.5, 0.1], [0.5, 0.2]]), 1.5);
        assert!((down[0].dir - std::f64::consts::FRAC_PI_2).abs() < 1e-9);
        assert_eq!(dabs(&stroke(vec![[0.3, 0.3]]), 1.5).len(), 1);
    }

    #[test]
    fn the_default_source_is_beside_the_stroke() {
        let (dx, dy) = HealStroke::default_offset(&[[0.1, 0.5], [0.3, 0.5]], 0.01, 1.5);
        assert!(
            dx.abs() < 1e-12 && (dy.abs() - 0.045).abs() < 1e-12,
            "{dx} {dy}"
        );
    }

    #[test]
    fn strokes_round_trip_and_bad_ones_are_dropped() {
        let mut s = EditSettings::default_settings();
        Heals(vec![stroke(vec![[0.1, 0.1], [0.2, 0.2]])]).write_into(&mut s);
        assert_eq!(Heals::from_settings(&s).0.len(), 1);
        let bad = serde_json::json!([{ "points": [], "r": 0.01, "dx": 0, "dy": 0 }]);
        s.desktop.as_mut().unwrap().insert("heal".into(), bad);
        assert!(Heals::from_settings(&s).0.is_empty());
        Heals::default().write_into(&mut s);
        assert!(s.desktop.is_none());
    }
}
