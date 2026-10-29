//! Upright: the vertical and horizontal perspective and the straightening that make a photo's
//! lines upright and level, as Lightroom's Upright does. The lines come from the photo
//! (found by the app); this module only fits the corrections to them.
//!
//! Each line is taken through the lens corrections ([`Lens::to_corrected`], the CPU twin of
//! the shader's) and then turned by the straightening angle, and should come out vertical (if
//! it was nearer vertical than level) or level. The cost is each line's leftover lean, squared
//! and capped (so a line that is truly slanted, a roof, cannot pull the fit), weighted by its
//! length. The search is coordinate descent over the three values, coarse then fine.
//!
//! Guided Upright ([`guided`]) fits the same corrections to lines the user drew: every guide
//! counts in full however far it leans (no cap: the user chose it), and the corrections
//! fitted follow what was drawn (two upright guides: the vertical perspective; two level
//! ones: the horizontal; both: both).

use crate::lens::Lens;

/// A line found in the photo: its ends (photo fractions, as shot) and how much it counts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    pub a: (f64, f64),
    pub b: (f64, f64),
    pub weight: f64,
}

/// What Upright corrects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Level, and verticals upright (vertical perspective).
    Vertical,
    /// Level, verticals upright and horizontals level (both perspectives).
    Full,
    /// Level, and horizontals level (horizontal perspective; Guided with level guides only).
    Horizontal,
}

/// The fitted corrections: the lens's perspective sliders and the straightening angle
/// (degrees, as `EditSettings::angle`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    pub vertical: f64,
    pub horizontal: f64,
    pub angle: f64,
}

/// The largest straightening Upright applies (degrees).
pub const MAX_ANGLE: f64 = 10.0;
/// How much of a line's lean (tan²) counts at most.
const CAP: f64 = 0.03;

fn cost(segments: &[Segment], aspect: f64, lens: &Lens, angle: f64, cap: f64) -> f64 {
    let (s, c) = angle.to_radians().sin_cos();
    let mut total = 0.0;
    for seg in segments {
        let p = lens.to_corrected(seg.a, aspect);
        let q = lens.to_corrected(seg.b, aspect);
        // In pixels' proportions (x × aspect), y down, then turned as the geometry turns.
        let d = ((q.0 - p.0) * aspect, q.1 - p.1);
        let d = (c * d.0 - s * d.1, s * d.0 + c * d.1);
        let src = ((seg.b.0 - seg.a.0) * aspect, seg.b.1 - seg.a.1);
        let lean = if src.1.abs() >= src.0.abs() {
            (d.0 / d.1.abs().max(1e-9)).powi(2)
        } else {
            (d.1 / d.0.abs().max(1e-9)).powi(2)
        };
        total += seg.weight * lean.min(cap);
    }
    total
}

/// The value in `lo..=hi` (every `step`) with the lowest `f`.
fn best(lo: f64, hi: f64, step: f64, f: impl Fn(f64) -> f64) -> f64 {
    let n = ((hi - lo) / step).round() as i64;
    (0..=n)
        .map(|i| lo + i as f64 * step)
        .map(|v| (v, f(v)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map_or(0.0, |(v, _)| v)
}

/// The corrections that make `segments` upright and level on a photo `aspect` wide per unit
/// of height, keeping `base`'s other lens corrections. `None` when there are too few lines to
/// say (at least two near-vertical ones, and in `Full` two near-level ones too).
pub fn fit(segments: &[Segment], aspect: f64, base: &Lens, mode: Mode) -> Option<Fit> {
    fit_with(segments, aspect, base, mode, CAP)
}

/// Guided Upright: the corrections that make the user's `guides` upright or level (see the
/// module docs); `None` until there are two upright or two level guides.
pub fn guided(guides: &[Segment], aspect: f64, base: &Lens) -> Option<Fit> {
    let (verticals, levels) = counts(guides, aspect);
    let mode = match (verticals >= 2, levels >= 2) {
        (true, true) => Mode::Full,
        (true, false) => Mode::Vertical,
        (false, true) => Mode::Horizontal,
        (false, false) => return None,
    };
    fit_with(guides, aspect, base, mode, f64::INFINITY)
}

/// How many of `segments` are nearer upright than level, and how many are not.
fn counts(segments: &[Segment], aspect: f64) -> (usize, usize) {
    let verticals = segments
        .iter()
        .filter(|s| (s.b.1 - s.a.1).abs() >= ((s.b.0 - s.a.0) * aspect).abs())
        .count();
    (verticals, segments.len() - verticals)
}

fn fit_with(segments: &[Segment], aspect: f64, base: &Lens, mode: Mode, cap: f64) -> Option<Fit> {
    let (verticals, levels) = counts(segments, aspect);
    let enough = match mode {
        Mode::Vertical => verticals >= 2,
        Mode::Full => verticals >= 2 && levels >= 2,
        Mode::Horizontal => levels >= 2,
    };
    if !enough {
        return None;
    }
    let lens_of = |v: f64, h: f64| Lens {
        vertical: v,
        horizontal: h,
        ..*base
    };
    let mut f = Fit {
        vertical: 0.0,
        horizontal: 0.0,
        angle: 0.0,
    };
    for (span, step, angle_step) in [(100.0, 4.0, 0.5), (8.0, 0.5, 0.1), (1.0, 0.1, 0.02)] {
        for _ in 0..3 {
            if mode != Mode::Horizontal {
                let (lo, hi) = (f.vertical - span, f.vertical + span);
                f.vertical = best(lo.max(-100.0), hi.min(100.0), step, |v| {
                    cost(segments, aspect, &lens_of(v, f.horizontal), f.angle, cap)
                });
            }
            if mode != Mode::Vertical {
                let (lo, hi) = (f.horizontal - span, f.horizontal + span);
                f.horizontal = best(lo.max(-100.0), hi.min(100.0), step, |h| {
                    cost(segments, aspect, &lens_of(f.vertical, h), f.angle, cap)
                });
            }
            let a_span = (span / 10.0).max(angle_step * 10.0);
            let (lo, hi) = (f.angle - a_span, f.angle + a_span);
            f.angle = best(lo.max(-MAX_ANGLE), hi.min(MAX_ANGLE), angle_step, |a| {
                cost(segments, aspect, &lens_of(f.vertical, f.horizontal), a, cap)
            });
        }
    }
    f.vertical = (f.vertical * 10.0).round() / 10.0;
    f.horizontal = (f.horizontal * 10.0).round() / 10.0;
    f.angle = (f.angle * 100.0).round() / 100.0;
    Some(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where the photo as shot shows a corrected point (lines are made in the corrected,
    /// upright world and then "photographed" through the inverse correction).
    fn shoot(lens: &Lens, angle: f64, p: (f64, f64), aspect: f64) -> (f64, f64) {
        // Un-turn by the angle about the centre (pixel proportions), then into the source.
        let (s, c) = (-angle).to_radians().sin_cos();
        let (x, y) = ((p.0 - 0.5) * aspect, p.1 - 0.5);
        let (x, y) = (c * x - s * y, s * x + c * y);
        lens.to_source((x / aspect + 0.5, y + 0.5), aspect)
    }

    fn scene(lens: &Lens, angle: f64, aspect: f64) -> Vec<Segment> {
        let mut out = Vec::new();
        for x in [0.2, 0.35, 0.65, 0.8] {
            out.push(Segment {
                a: shoot(lens, angle, (x, 0.15), aspect),
                b: shoot(lens, angle, (x, 0.85), aspect),
                weight: 1.0,
            });
        }
        for y in [0.25, 0.75] {
            out.push(Segment {
                a: shoot(lens, angle, (0.2, y), aspect),
                b: shoot(lens, angle, (0.8, y), aspect),
                weight: 1.0,
            });
        }
        out
    }

    #[test]
    fn converging_verticals_and_a_tilt_are_found_again() {
        let aspect = 1.5;
        let truth = Lens {
            vertical: 35.0,
            ..Lens::default()
        };
        let segs = scene(&truth, 2.0, aspect);
        let f = fit(&segs, aspect, &Lens::default(), Mode::Vertical).unwrap();
        assert!((f.vertical - 35.0).abs() < 1.5, "{f:?}");
        assert!((f.angle - 2.0).abs() < 0.3, "{f:?}");
        assert_eq!(f.horizontal, 0.0);
    }

    #[test]
    fn full_finds_the_horizontal_perspective_too() {
        let aspect = 1.5;
        let truth = Lens {
            vertical: -20.0,
            horizontal: 25.0,
            ..Lens::default()
        };
        let segs = scene(&truth, 0.0, aspect);
        let f = fit(&segs, aspect, &Lens::default(), Mode::Full).unwrap();
        assert!((f.vertical + 20.0).abs() < 2.0, "{f:?}");
        assert!((f.horizontal - 25.0).abs() < 2.0, "{f:?}");
        assert!(f.angle.abs() < 0.3, "{f:?}");
    }

    #[test]
    fn guides_count_in_full_and_choose_what_is_fitted() {
        let aspect = 1.5;
        // A strong lean, beyond what automatic Upright trusts a line with.
        let truth = Lens {
            vertical: 80.0,
            ..Lens::default()
        };
        let all = scene(&truth, 0.0, aspect);
        // Two upright guides (the outer edges of a building).
        let two = [all[0], all[3]];
        let g = guided(&two, aspect, &Lens::default()).unwrap();
        assert!((g.vertical - 80.0).abs() < 3.0, "{g:?}");
        assert_eq!(g.horizontal, 0.0, "only what was drawn is fitted");
        // Two level guides alone fit the horizontal perspective.
        let truth = Lens {
            horizontal: -30.0,
            ..Lens::default()
        };
        let all = scene(&truth, 0.0, aspect);
        let g = guided(&all[4..6], aspect, &Lens::default()).unwrap();
        assert!((g.horizontal + 30.0).abs() < 3.0, "{g:?}");
        assert_eq!(g.vertical, 0.0);
        // One guide: not yet.
        assert_eq!(guided(&all[..1], aspect, &Lens::default()), None);
    }

    #[test]
    fn too_few_lines_say_nothing() {
        let one = [Segment {
            a: (0.5, 0.1),
            b: (0.5, 0.9),
            weight: 1.0,
        }];
        assert_eq!(fit(&one, 1.5, &Lens::default(), Mode::Vertical), None);
    }
}
