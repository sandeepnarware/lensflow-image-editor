//! The spot tool (`Q`): heal and clone spots, as the web's `HealOverlay.vue`.
//!
//! **Coordinates and radius.** A spot's centres `(x, y)` (the spot) and `(sx, sy)` (where its
//! fix is copied from) are UV of the source photo: fractions of the file's own width and
//! height, before quarter turns, flips, straighten and crop (the spots pass runs first, on
//! the whole photo). The radius `r` is a fraction of the photo's **width**, not of its short
//! side: the web's `shaders.ts` SPOTS pass (and `lf-pipeline`'s `spots.wgsl`, ported from it)
//! computes `r = uSpotB[i].x * uSize.x` in pixels and measures the distance
//! `length((vUv - s.xy) * uSize)` in pixels too, so a spot is a true circle `r × width`
//! pixels across whatever the photo's shape. In UV, that circle is `r` wide and
//! `r × aspect` tall (`aspect` = width ÷ height), which is what [`hit`] measures.
//!
//! Commits: each spot change is one history step, as the web commits per spot: placing one
//! (on release), moving either circle (on release), resizing with the wheel (once the wheel
//! rests), removing one, and switching one's mode.

use super::ToolOutput;
use lf_settings::{MAX_SPOTS, Spot, SpotMode};

/// The smallest radius a drag places (a fraction of the photo's width).
pub const MIN_RADIUS: f64 = 0.005;
/// The radius a click without a drag places (the web's default brush, 2 % of the width).
pub const CLICK_RADIUS: f64 = 0.02;
/// The largest radius (the web's brush slider stops at 15 %).
pub const MAX_RADIUS: f64 = 0.15;
/// Shown when a photo already has [`MAX_SPOTS`] spots.
pub const FULL: &str = "Up to 48 spots per photo.";
pub const HEAL: &str = "Heal";
pub const CLONE: &str = "Clone";
pub const REMOVE: &str = "Remove spot";
pub const HINT: &str = "Click a blemish to cover it, or drag to size the spot. Drag either circle to move it; scroll over one to resize it.";

/// Which circle of a spot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// The spot itself.
    Target,
    /// Where its fix comes from.
    Source,
}

/// The distance from `a` to `b` (source UV) in fractions of the photo's width.
pub fn distance(a: (f64, f64), b: (f64, f64), aspect: f64) -> f64 {
    (a.0 - b.0).hypot((a.1 - b.1) / aspect)
}

/// The spot circle under `p` (source UV) on a photo `aspect` (width ÷ height) wide: sources
/// first (they are drawn on top), the newest spot first.
pub fn hit(spots: &[Spot], p: (f64, f64), aspect: f64) -> Option<(usize, Part)> {
    hit_within(spots, p, aspect, 0.0)
}

/// [`hit`], with every circle at least `min_r` (fraction of the width) for grabbing: a tiny
/// spot stays easy to pick up on screen.
pub fn hit_within(spots: &[Spot], p: (f64, f64), aspect: f64, min_r: f64) -> Option<(usize, Part)> {
    for part in [Part::Source, Part::Target] {
        for (i, s) in spots.iter().enumerate().rev() {
            let centre = match part {
                Part::Source => (s.sx, s.sy),
                Part::Target => (s.x, s.y),
            };
            if distance(centre, p, aspect) <= s.r.max(min_r) {
                return Some((i, part));
            }
        }
    }
    None
}

/// Where a new spot's fix comes from by default: `2r` to its right, kept inside the photo
/// (to its left when the right has no room).
pub fn default_source(x: f64, y: f64, r: f64, aspect: f64) -> (f64, f64) {
    let ry = r * aspect;
    let (lo, hi) = (r.min(0.5), (1.0 - r).max(0.5));
    let mut sx = x + 2.0 * r;
    if sx > hi {
        sx = x - 2.0 * r;
    }
    (sx.clamp(lo, hi), y.clamp(ry.min(0.5), (1.0 - ry).max(0.5)))
}

/// Adds a spot at `(x, y)` with radius `r`; its index, or [`FULL`] at [`MAX_SPOTS`].
pub fn place(
    spots: &mut Vec<Spot>,
    (x, y): (f64, f64),
    r: f64,
    mode: SpotMode,
    aspect: f64,
) -> Result<usize, &'static str> {
    if spots.len() >= MAX_SPOTS {
        return Err(FULL);
    }
    let r = r.clamp(MIN_RADIUS, MAX_RADIUS);
    let (sx, sy) = default_source(x, y, r, aspect);
    spots.push(Spot {
        x,
        y,
        sx,
        sy,
        r,
        mode,
    });
    Ok(spots.len() - 1)
}

/// The radius for a drag from `centre` to `p`, within [`MIN_RADIUS`]..[`MAX_RADIUS`].
pub fn radius_between(centre: (f64, f64), p: (f64, f64), aspect: f64) -> f64 {
    distance(centre, p, aspect).clamp(MIN_RADIUS, MAX_RADIUS)
}

fn unit(v: f64) -> f64 {
    v.clamp(0.0, 1.0)
}

/// Moves the spot to `to`; its source follows, keeping its offset (clamped to the photo).
pub fn move_target(spot: &mut Spot, to: (f64, f64)) {
    let (x, y) = (unit(to.0), unit(to.1));
    spot.sx = unit(spot.sx + x - spot.x);
    spot.sy = unit(spot.sy + y - spot.y);
    spot.x = x;
    spot.y = y;
}

/// Moves the spot's source to `to` (clamped to the photo).
pub fn move_source(spot: &mut Spot, to: (f64, f64)) {
    spot.sx = unit(to.0);
    spot.sy = unit(to.1);
}

/// Scales the spot's radius by `factor`, within [`MIN_RADIUS`]..[`MAX_RADIUS`].
pub fn resize(spot: &mut Spot, factor: f64) {
    spot.r = (spot.r * factor).clamp(MIN_RADIUS, MAX_RADIUS);
}

/// A drag in the spot tool.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SpotDrag {
    /// Sizing a new spot from its centre.
    Placing { index: usize, centre: (f64, f64) },
    /// Moving a circle; `grab` is the circle's centre minus where it was grabbed.
    Moving {
        index: usize,
        part: Part,
        grab: (f64, f64),
        moved: bool,
    },
}

/// The open spot tool.
#[derive(Debug, Clone)]
pub struct SpotTool {
    /// The spot `Delete` removes and the panel's mode applies to.
    pub selected: Option<usize>,
    /// The mode new spots get (the selected spot's, once one is picked).
    pub mode: SpotMode,
    pub drag: Option<SpotDrag>,
    /// The wheel resized a spot at this time (seconds, egui's clock): committed once it rests.
    pub resized_at: Option<f64>,
}

/// How long the wheel rests before a resize is committed (seconds).
pub const WHEEL_REST: f64 = 0.4;

impl SpotTool {
    pub fn new(mode: SpotMode) -> SpotTool {
        SpotTool {
            selected: None,
            mode,
            drag: None,
            resized_at: None,
        }
    }

    /// Picks spot `index` (its mode shows in the panel).
    pub fn select(&mut self, spots: &[Spot], index: Option<usize>) {
        self.selected = index.filter(|&i| i < spots.len());
        if let Some(i) = self.selected {
            self.mode = spots[i].mode;
        }
    }

    /// A press at `p` (source UV): grabs the circle there, or starts a new spot. `Err(FULL)`
    /// when a new spot would be one too many. `min_r` keeps tiny circles grabbable.
    pub fn press(
        &mut self,
        spots: &mut Vec<Spot>,
        p: (f64, f64),
        aspect: f64,
        min_r: f64,
        radius: f64,
    ) -> Result<ToolOutput, &'static str> {
        if let Some((index, part)) = hit_within(spots, p, aspect, min_r) {
            let s = spots[index];
            let centre = match part {
                Part::Target => (s.x, s.y),
                Part::Source => (s.sx, s.sy),
            };
            self.select(spots, Some(index));
            self.drag = Some(SpotDrag::Moving {
                index,
                part,
                grab: (centre.0 - p.0, centre.1 - p.1),
                moved: false,
            });
            return Ok(ToolOutput::default());
        }
        if !(0.0..=1.0).contains(&p.0) || !(0.0..=1.0).contains(&p.1) {
            return Ok(ToolOutput::default());
        }
        let index = place(spots, p, radius, self.mode, aspect)?;
        self.selected = Some(index);
        self.drag = Some(SpotDrag::Placing { index, centre: p });
        Ok(ToolOutput {
            changed: true,
            commit: false,
        })
    }

    /// The pointer moved to `p` during a drag. True when a spot changed.
    pub fn drag_to(&mut self, spots: &mut [Spot], p: (f64, f64), aspect: f64) -> bool {
        match &mut self.drag {
            Some(SpotDrag::Placing { index, centre }) => {
                let Some(spot) = spots.get_mut(*index) else {
                    return false;
                };
                spot.r = radius_between(*centre, p, aspect);
                let (sx, sy) = default_source(spot.x, spot.y, spot.r, aspect);
                spot.sx = sx;
                spot.sy = sy;
                true
            }
            Some(SpotDrag::Moving {
                index,
                part,
                grab,
                moved,
            }) => {
                let Some(spot) = spots.get_mut(*index) else {
                    return false;
                };
                let to = (p.0 + grab.0, p.1 + grab.1);
                match part {
                    Part::Target => move_target(spot, to),
                    Part::Source => move_source(spot, to),
                }
                *moved = true;
                true
            }
            None => false,
        }
    }

    /// The press ended: a new spot, or a circle that moved, is one history step.
    pub fn release(&mut self) -> ToolOutput {
        let commit = match self.drag.take() {
            Some(SpotDrag::Placing { .. }) => true,
            Some(SpotDrag::Moving { moved, .. }) => moved,
            None => false,
        };
        ToolOutput {
            changed: false,
            commit,
        }
    }

    /// The wheel over spot `index`: resized by `factor`, committed once the wheel rests.
    pub fn wheel(&mut self, spots: &mut [Spot], index: usize, factor: f64, now: f64) -> bool {
        let Some(spot) = spots.get_mut(index) else {
            return false;
        };
        let before = spot.r;
        resize(spot, factor);
        self.selected = Some(index);
        self.mode = spot.mode;
        if spot.r != before {
            self.resized_at = Some(now);
            return true;
        }
        false
    }

    /// The wheel has rested since the last resize: commit it.
    pub fn wheel_rested(&mut self, now: f64) -> bool {
        match self.resized_at {
            Some(at) if now - at >= WHEEL_REST => {
                self.resized_at = None;
                true
            }
            _ => false,
        }
    }

    /// `Delete`: removes the selected spot (one history step).
    pub fn remove_selected(&mut self, spots: &mut Vec<Spot>) -> ToolOutput {
        let Some(i) = self.selected.take().filter(|&i| i < spots.len()) else {
            return ToolOutput::default();
        };
        spots.remove(i);
        self.drag = None;
        ToolOutput {
            changed: true,
            commit: true,
        }
    }

    /// The panel's Heal / Clone: new spots get `mode`, and so does the selected one (one
    /// history step when it changed).
    pub fn set_mode(&mut self, spots: &mut [Spot], mode: SpotMode) -> ToolOutput {
        self.mode = mode;
        match self.selected.and_then(|i| spots.get_mut(i)) {
            Some(spot) if spot.mode != mode => {
                spot.mode = mode;
                ToolOutput {
                    changed: true,
                    commit: true,
                }
            }
            _ => ToolOutput::default(),
        }
    }

    /// The spots changed under the tool (undo, redo): nothing stays picked or held.
    pub fn clear_selection(&mut self) {
        self.selected = None;
        self.drag = None;
        self.resized_at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spot(x: f64, y: f64, sx: f64, sy: f64, r: f64) -> Spot {
        Spot {
            x,
            y,
            sx,
            sy,
            r,
            mode: SpotMode::Heal,
        }
    }

    /// A 3:2 landscape photo: a circle of radius r is r wide and 1.5 r tall in UV.
    const ASPECT: f64 = 1.5;

    #[test]
    fn spot_hit_prefers_source_handle_when_overlapping() {
        // Spot 0's source overlaps its own target.
        let spots = [spot(0.5, 0.5, 0.53, 0.5, 0.02)];
        assert_eq!(
            hit(&spots, (0.515, 0.5), ASPECT),
            Some((0, Part::Source)),
            "inside both circles: the source wins"
        );
        assert_eq!(hit(&spots, (0.485, 0.5), ASPECT), Some((0, Part::Target)));
        assert_eq!(hit(&spots, (0.6, 0.5), ASPECT), None);

        // Radii are fractions of the width: in a 3:2 photo the circle reaches 1.5 r in v.
        let spots = [spot(0.5, 0.5, 0.9, 0.9, 0.02)];
        assert_eq!(hit(&spots, (0.5, 0.529), ASPECT), Some((0, Part::Target)));
        assert_eq!(hit(&spots, (0.5, 0.531), ASPECT), None);
        assert_eq!(hit(&spots, (0.519, 0.5), ASPECT), Some((0, Part::Target)));
        assert_eq!(hit(&spots, (0.521, 0.5), ASPECT), None);
        // A portrait photo (2:3): the circle is shorter than wide in v.
        assert_eq!(
            hit(&spots, (0.5, 0.513), 2.0 / 3.0),
            Some((0, Part::Target))
        );
        assert_eq!(hit(&spots, (0.5, 0.514), 2.0 / 3.0), None);

        // Spot 1's source over spot 0's target: sources are on top.
        let spots = [
            spot(0.3, 0.3, 0.8, 0.8, 0.03),
            spot(0.1, 0.1, 0.31, 0.3, 0.03),
        ];
        assert_eq!(hit(&spots, (0.305, 0.3), ASPECT), Some((1, Part::Source)));
        // A tiny spot is still grabbable within min_r.
        let spots = [spot(0.5, 0.5, 0.9, 0.5, 0.001)];
        assert_eq!(hit(&spots, (0.504, 0.5), ASPECT), None);
        assert_eq!(
            hit_within(&spots, (0.504, 0.5), ASPECT, 0.005),
            Some((0, Part::Target))
        );
    }

    #[test]
    fn max_spots_refused() {
        let mut spots = Vec::new();
        for i in 0..MAX_SPOTS {
            let at = (0.1 + 0.015 * i as f64, 0.5);
            assert_eq!(place(&mut spots, at, 0.01, SpotMode::Heal, ASPECT), Ok(i));
        }
        assert_eq!(
            place(&mut spots, (0.5, 0.2), 0.01, SpotMode::Clone, ASPECT),
            Err(FULL)
        );
        assert_eq!(spots.len(), MAX_SPOTS);
        assert_eq!(FULL, "Up to 48 spots per photo.");

        // The tool says so on a press in empty space, and still grabs existing circles.
        let mut tool = SpotTool::new(SpotMode::Heal);
        assert_eq!(
            tool.press(&mut spots, (0.5, 0.9), ASPECT, 0.0, CLICK_RADIUS),
            Err(FULL)
        );
        assert!(
            tool.press(&mut spots, (0.1, 0.5), ASPECT, 0.0, CLICK_RADIUS)
                .is_ok()
        );
        assert_eq!(tool.selected, Some(0));
    }

    #[test]
    fn a_new_spot_copies_from_two_radii_right_inside_the_photo() {
        let mut spots = Vec::new();
        let i = place(&mut spots, (0.4, 0.5), 0.02, SpotMode::Clone, ASPECT).unwrap();
        let s = spots[i];
        assert!(
            (s.sx - 0.44).abs() < 1e-9 && (s.sy - 0.5).abs() < 1e-9,
            "{s:?}"
        );
        assert_eq!(s.mode, SpotMode::Clone);
        // At the right edge there is no room: it copies from the left.
        let (sx, _) = default_source(0.97, 0.5, 0.02, ASPECT);
        assert!((sx - 0.93).abs() < 1e-9, "{sx}");
        // Near the top, the source is kept inside (r × aspect from the edge in v).
        let (_, sy) = default_source(0.5, 0.0, 0.02, ASPECT);
        assert!((sy - 0.03).abs() < 1e-9, "{sy}");
    }

    #[test]
    fn placing_by_drag_sizes_the_spot_and_commits_once_on_release() {
        let mut spots = Vec::new();
        let mut tool = SpotTool::new(SpotMode::Heal);
        let out = tool
            .press(&mut spots, (0.5, 0.5), ASPECT, 0.0, MIN_RADIUS)
            .unwrap();
        assert!(out.changed && !out.commit);
        assert!(tool.drag_to(&mut spots, (0.53, 0.5), ASPECT));
        assert!((spots[0].r - 0.03).abs() < 1e-9);
        assert!(
            (spots[0].sx - 0.56).abs() < 1e-9,
            "the source follows the size"
        );
        assert!(tool.drag_to(&mut spots, (0.5, 0.5001), ASPECT));
        assert_eq!(spots[0].r, MIN_RADIUS, "never below the minimum");
        assert!(tool.release().commit);

        // Moving the target carries the source; a press without a move commits nothing.
        let source = (spots[0].sx, spots[0].sy);
        tool.press(&mut spots, (0.5, 0.5), ASPECT, 0.0, CLICK_RADIUS)
            .unwrap();
        assert!(!tool.release().commit, "a click that only selects");
        tool.press(&mut spots, (0.5, 0.5), ASPECT, 0.0, CLICK_RADIUS)
            .unwrap();
        tool.drag_to(&mut spots, (0.6, 0.4), ASPECT);
        assert!(tool.release().commit);
        assert!((spots[0].x - 0.6).abs() < 1e-9 && (spots[0].y - 0.4).abs() < 1e-9);
        assert!((spots[0].sx - (source.0 + 0.1)).abs() < 1e-9);
        assert!((spots[0].sy - (source.1 - 0.1)).abs() < 1e-9);

        // Delete removes the selected one.
        assert_eq!(tool.selected, Some(0));
        assert!(tool.remove_selected(&mut spots).commit);
        assert!(spots.is_empty() && tool.selected.is_none());
    }

    #[test]
    fn the_wheel_resizes_and_commits_once_it_rests() {
        let mut spots = vec![spot(0.5, 0.5, 0.6, 0.5, 0.02)];
        let mut tool = SpotTool::new(SpotMode::Heal);
        assert!(tool.wheel(&mut spots, 0, 1.5, 10.0));
        assert!(tool.wheel(&mut spots, 0, 1.5, 10.1));
        assert!((spots[0].r - 0.045).abs() < 1e-9);
        assert!(!tool.wheel_rested(10.3));
        assert!(tool.wheel_rested(10.6));
        assert!(!tool.wheel_rested(11.0), "once");
        assert!(tool.wheel(&mut spots, 0, 100.0, 12.0));
        assert_eq!(spots[0].r, MAX_RADIUS);

        // The mode switch applies to the selected spot.
        let out = tool.set_mode(&mut spots, SpotMode::Clone);
        assert!(out.commit && spots[0].mode == SpotMode::Clone);
        assert!(
            !tool.set_mode(&mut spots, SpotMode::Clone).commit,
            "unchanged"
        );
    }
}
