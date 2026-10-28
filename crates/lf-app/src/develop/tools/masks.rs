//! The masks tool (`M`): adds linear and radial gradients and moves them by their handles on
//! the photo. The logic here works in photo fractions (the space the masks are stored in) and
//! is told the photo's aspect; the Develop view maps the pointer in and the handles out.
//!
//! A linear gradient has a handle on its start and its end line (each moves that end) and its
//! middle line (moves the whole). A radial has its centre (moves it), a handle on its right
//! and one on its bottom edge (each sets that radius, along the ellipse's own axes), and its
//! inside (moves it).
//!
//! A brush mask is painted: while one is picked, a drag lays a stroke along the pointer with the
//! brush's size, softness and flow ([`BrushTip`]), or takes one away when erasing (the
//! panel's "Erase", or Alt held). A click is a single dab.

use lf_settings::masks::{MAX_MASKS, MAX_POINTS, MAX_STROKES, Mask, Masks, Shape, Stroke};

/// A point kept to four places (a tenth of a pixel on a 10,000-pixel photo), so a sidecar
/// holding many strokes stays small.
fn rounded((u, v): (f64, f64)) -> [f64; 2] {
    [(u * 1e4).round() / 1e4, (v * 1e4).round() / 1e4]
}

/// The hint over the masks panel.
pub const HINT: &str = "Add a gradient and drag its handles, or a brush and paint on the \
                        photo. Each mask has its own sliders.";
pub const ADD_LINEAR: &str = "Add linear";
pub const ADD_RADIAL: &str = "Add radial";
pub const ADD_BRUSH: &str = "Add brush";
pub const BRUSH_HINT: &str = "Paint on the photo. Hold Alt to erase.";
pub const ERASE: &str = "Erase";
pub const REMOVE: &str = "Remove mask";
pub const INVERT: &str = "Invert";
/// Before the people buttons (People, Face skin, Body skin, Hair, Clothes).
pub const PEOPLE: &str = "People:";
pub const AI_OFF: &str = "The AI tools need the studio in good standing.";
pub const RANGE: &str = "Range";
pub const RANGE_PICK: &str = "Alt-click the photo to pick the colour.";
pub const NO_HUE: &str = "That spot has no colour to pick (it is grey); try a more colourful part.";
pub const FULL: &str = "A photo can have up to 8 masks.";

/// What a press caught.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Grab {
    LinearStart,
    LinearEnd,
    /// The whole gradient, from where it was caught.
    LinearBody((f64, f64)),
    RadialCentre,
    /// The radius along the ellipse's own x axis.
    RadialX,
    RadialY,
    RadialBody((f64, f64)),
}

/// The brush a stroke is laid with: `size` across on each side (a fraction of the photo's
/// height), its outer `feather` percent soft, at `flow` percent; `erase` takes away instead.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrushTip {
    pub size: f64,
    pub feather: f64,
    pub flow: f64,
    pub erase: bool,
}

impl Default for BrushTip {
    fn default() -> BrushTip {
        BrushTip {
            size: 0.05,
            feather: 50.0,
            flow: 100.0,
            erase: false,
        }
    }
}

/// The tool's state: the mask being edited, the handle held, and the stroke being painted.
#[derive(Debug, Clone, Default)]
pub struct MaskTool {
    pub selected: Option<usize>,
    pub grab: Option<(usize, Grab)>,
    pub brush: BrushTip,
    /// The brush mask a stroke is being laid on (its last stroke).
    pub painting: Option<usize>,
    /// The AI buttons may be used (the studio in good standing).
    pub ai_allowed: bool,
    /// An AI mask's button was pressed: what it should find.
    pub ai_request: Option<lf_ai::segment::Target>,
}

/// A point in the square space (x × aspect), where distances mean the same both ways.
fn sq((u, v): (f64, f64), aspect: f64) -> (f64, f64) {
    (u * aspect, v)
}

fn unsq((x, y): (f64, f64), aspect: f64) -> (f64, f64) {
    (x / aspect, y)
}

/// A radial's handles in photo fractions: centre, the x-radius end and the y-radius end.
pub fn radial_handles(shape: &Shape, aspect: f64) -> Option<[(f64, f64); 3]> {
    let Shape::Radial {
        cx,
        cy,
        rx,
        ry,
        angle,
        ..
    } = *shape
    else {
        return None;
    };
    let (s, c) = angle.to_radians().sin_cos();
    let centre = sq((cx, cy), aspect);
    let at = |dx: f64, dy: f64| {
        unsq(
            (centre.0 + dx * c - dy * s, centre.1 + dx * s + dy * c),
            aspect,
        )
    };
    Some([(cx, cy), at(rx, 0.0), at(0.0, ry)])
}

/// The ellipse's outline in photo fractions (for drawing), `n` points.
pub fn radial_outline(shape: &Shape, aspect: f64, n: usize) -> Vec<(f64, f64)> {
    let Shape::Radial {
        cx,
        cy,
        rx,
        ry,
        angle,
        ..
    } = *shape
    else {
        return Vec::new();
    };
    let (s, c) = angle.to_radians().sin_cos();
    let centre = sq((cx, cy), aspect);
    (0..n)
        .map(|i| {
            let t = i as f64 / n as f64 * std::f64::consts::TAU;
            let (dx, dy) = (rx * t.cos(), ry * t.sin());
            unsq(
                (centre.0 + dx * c - dy * s, centre.1 + dx * s + dy * c),
                aspect,
            )
        })
        .collect()
}

/// A linear gradient's three lines, each as two ends in photo fractions: start, middle, end.
/// Each runs across the gradient's direction, `half` (square units) to either side.
pub fn linear_lines(shape: &Shape, aspect: f64, half: f64) -> Option<[[(f64, f64); 2]; 3]> {
    let Shape::Linear { x0, y0, x1, y1 } = *shape else {
        return None;
    };
    let (a, b) = (sq((x0, y0), aspect), sq((x1, y1), aspect));
    let d = (b.0 - a.0, b.1 - a.1);
    let len = d.0.hypot(d.1).max(1e-9);
    let n = (-d.1 / len * half, d.0 / len * half);
    let line = |p: (f64, f64)| {
        [
            unsq((p.0 - n.0, p.1 - n.1), aspect),
            unsq((p.0 + n.0, p.1 + n.1), aspect),
        ]
    };
    let mid = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
    Some([line(a), line(mid), line(b)])
}

impl MaskTool {
    /// Adds `mask` (refused past [`MAX_MASKS`]) and selects it.
    pub fn add(&mut self, masks: &mut Masks, mask: Mask) -> Result<(), &'static str> {
        if masks.0.len() >= MAX_MASKS {
            return Err(FULL);
        }
        masks.0.push(mask);
        self.selected = Some(masks.0.len() - 1);
        Ok(())
    }

    /// Removes the selected mask; true when one was removed.
    pub fn remove_selected(&mut self, masks: &mut Masks) -> bool {
        let Some(i) = self.selected.filter(|&i| i < masks.0.len()) else {
            return false;
        };
        masks.0.remove(i);
        self.selected = masks.0.len().checked_sub(1).map(|last| i.min(last));
        self.grab = None;
        true
    }

    /// A press at `p` (photo fractions): the nearest handle within `reach` (square units)
    /// of the selected mask first, then of any mask; else inside a radial. Selects what it
    /// catches; false when it caught nothing.
    pub fn press(&mut self, masks: &Masks, p: (f64, f64), aspect: f64, reach: f64) -> bool {
        let order: Vec<usize> = self
            .selected
            .into_iter()
            .chain((0..masks.0.len()).rev())
            .filter(|&i| i < masks.0.len())
            .collect();
        let dist = |a: (f64, f64), b: (f64, f64)| {
            let (a, b) = (sq(a, aspect), sq(b, aspect));
            (a.0 - b.0).hypot(a.1 - b.1)
        };
        for &i in &order {
            let shape = &masks.0[i].shape;
            let grab = match shape {
                Shape::Linear { x0, y0, x1, y1 } => {
                    let mid = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
                    [
                        (Grab::LinearStart, (*x0, *y0)),
                        (Grab::LinearEnd, (*x1, *y1)),
                        (Grab::LinearBody(p), mid),
                    ]
                    .into_iter()
                    .filter(|(_, at)| dist(*at, p) <= reach)
                    .min_by(|a, b| dist(a.1, p).total_cmp(&dist(b.1, p)))
                    .map(|(g, _)| g)
                }
                Shape::Radial { .. } => {
                    let [c, x, y] = radial_handles(shape, aspect).expect("a radial");
                    [
                        (Grab::RadialCentre, c),
                        (Grab::RadialX, x),
                        (Grab::RadialY, y),
                    ]
                    .into_iter()
                    .filter(|(_, at)| dist(*at, p) <= reach)
                    .min_by(|a, b| dist(a.1, p).total_cmp(&dist(b.1, p)))
                    .map(|(g, _)| g)
                }
                Shape::Brush { .. } | Shape::Image { .. } => None,
            };
            if let Some(grab) = grab {
                self.selected = Some(i);
                self.grab = Some((i, grab));
                return true;
            }
        }
        // Inside a radial (the selected one first) moves it.
        for &i in &order {
            let inside = |m: &Mask| {
                let solid = Mask {
                    invert: false,
                    ..m.clone()
                };
                matches!(m.shape, Shape::Radial { .. })
                    && lf_settings::masks::weight(&solid, p, aspect) > 0.0
            };
            if inside(&masks.0[i]) {
                self.selected = Some(i);
                self.grab = Some((i, Grab::RadialBody(p)));
                return true;
            }
        }
        false
    }

    /// The held handle follows the pointer to `p`; true when the masks changed.
    pub fn drag_to(&mut self, masks: &mut Masks, p: (f64, f64), aspect: f64) -> bool {
        let Some((i, grab)) = self.grab else {
            return false;
        };
        let Some(mask) = masks.0.get_mut(i) else {
            return false;
        };
        let before = mask.shape.clone();
        match (&mut mask.shape, grab) {
            (Shape::Linear { x0, y0, .. }, Grab::LinearStart) => (*x0, *y0) = p,
            (Shape::Linear { x1, y1, .. }, Grab::LinearEnd) => (*x1, *y1) = p,
            (Shape::Linear { x0, y0, x1, y1 }, Grab::LinearBody(from)) => {
                let d = (p.0 - from.0, p.1 - from.1);
                (*x0, *y0, *x1, *y1) = (*x0 + d.0, *y0 + d.1, *x1 + d.0, *y1 + d.1);
                self.grab = Some((i, Grab::LinearBody(p)));
            }
            (Shape::Radial { cx, cy, .. }, Grab::RadialCentre) => (*cx, *cy) = p,
            (Shape::Radial { cx, cy, .. }, Grab::RadialBody(from)) => {
                (*cx, *cy) = (*cx + p.0 - from.0, *cy + p.1 - from.1);
                self.grab = Some((i, Grab::RadialBody(p)));
            }
            (
                Shape::Radial {
                    cx,
                    cy,
                    rx,
                    ry,
                    angle,
                    ..
                },
                Grab::RadialX | Grab::RadialY,
            ) => {
                // The pointer's offset along the ellipse's own axes.
                let (s, c) = angle.to_radians().sin_cos();
                let (dx, dy) = (p.0 * aspect - *cx * aspect, p.1 - *cy);
                let along_x = dx * c + dy * s;
                let along_y = -dx * s + dy * c;
                if grab == Grab::RadialX {
                    *rx = along_x.abs().max(0.005);
                } else {
                    *ry = along_y.abs().max(0.005);
                }
            }
            _ => {}
        }
        // A linear gradient whose ends meet has no direction: keep the last good one.
        if let Shape::Linear { x0, y0, x1, y1 } = mask.shape
            && ((x1 - x0) * aspect).hypot(y1 - y0) < 1e-3
        {
            mask.shape = before;
            return false;
        }
        mask.shape != before
    }

    /// The picked mask, when it is a brush (a drag paints it rather than catching handles).
    pub fn brush_selected(&self, masks: &Masks) -> Option<usize> {
        self.selected
            .filter(|&i| matches!(masks.0.get(i).map(|m| &m.shape), Some(Shape::Brush { .. })))
    }

    /// Starts a stroke at `p` on the picked brush mask; `erase` (Alt held) takes away
    /// whatever the panel says. True when a stroke was started; false with no brush picked
    /// or at [`MAX_STROKES`].
    pub fn start_stroke(&mut self, masks: &mut Masks, p: (f64, f64), erase: bool) -> bool {
        let Some(i) = self.brush_selected(masks) else {
            return false;
        };
        let Shape::Brush { strokes } = &mut masks.0[i].shape else {
            return false;
        };
        if strokes.len() >= MAX_STROKES {
            return false;
        }
        strokes.push(Stroke {
            points: vec![rounded(p)],
            size: self.brush.size,
            feather: self.brush.feather,
            flow: self.brush.flow,
            erase: self.brush.erase != erase,
        });
        self.painting = Some(i);
        true
    }

    /// The stroke being painted reaches `p`: a point is added once the pointer is a fifth of
    /// the brush's size from the last (the line between them is painted, so nothing is
    /// lost). True when the mask changed.
    pub fn extend_stroke(&mut self, masks: &mut Masks, p: (f64, f64), aspect: f64) -> bool {
        let Some(Shape::Brush { strokes }) = self
            .painting
            .and_then(|i| masks.0.get_mut(i))
            .map(|m| &mut m.shape)
        else {
            return false;
        };
        let Some(stroke) = strokes.last_mut() else {
            return false;
        };
        let last = *stroke.points.last().expect("a stroke has a point");
        let moved = ((p.0 - last[0]) * aspect).hypot(p.1 - last[1]);
        if moved < stroke.size * 0.2 || stroke.points.len() >= MAX_POINTS {
            return false;
        }
        stroke.points.push(rounded(p));
        true
    }

    /// The button came up: the gesture is over (the caller commits it).
    pub fn release(&mut self) -> bool {
        let held = self.grab.take().is_some();
        self.painting.take().is_some() || held
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn radial() -> Mask {
        Mask::radial()
    }

    #[test]
    fn add_selects_and_the_ninth_is_refused() {
        let mut tool = MaskTool::default();
        let mut masks = Masks::default();
        tool.add(&mut masks, radial()).unwrap();
        assert_eq!(tool.selected, Some(0));
        for _ in 1..MAX_MASKS {
            tool.add(&mut masks, Mask::linear()).unwrap();
        }
        assert_eq!(tool.add(&mut masks, radial()), Err(FULL));
        assert_eq!(masks.0.len(), MAX_MASKS);
        assert_eq!(tool.selected, Some(MAX_MASKS - 1));
        // Removing the last selects the one before it; removing all selects nothing.
        assert!(tool.remove_selected(&mut masks));
        assert_eq!(tool.selected, Some(MAX_MASKS - 2));
        while tool.remove_selected(&mut masks) {}
        assert!(masks.0.is_empty());
        assert_eq!(tool.selected, None);
    }

    #[test]
    fn a_radial_moves_by_its_centre_and_inside_and_resizes_by_its_edges() {
        let aspect = 1.5;
        let mut tool = MaskTool::default();
        let mut masks = Masks(vec![radial()]);
        let [_, x_handle, y_handle] = radial_handles(&masks.0[0].shape, aspect).unwrap();
        // The x-radius handle, dragged out: rx grows (in units of the photo's height).
        assert!(tool.press(&masks, x_handle, aspect, 0.02));
        assert_eq!(tool.grab, Some((0, Grab::RadialX)));
        assert!(tool.drag_to(&mut masks, (0.5 + 0.4 / aspect, 0.5), aspect));
        let Shape::Radial { rx, ry, .. } = masks.0[0].shape else {
            panic!()
        };
        assert!(
            (rx - 0.4).abs() < 1e-9 && (ry - 0.2).abs() < 1e-9,
            "{rx} {ry}"
        );
        assert!(tool.release());
        // The y handle.
        assert!(tool.press(&masks, y_handle, aspect, 0.02));
        tool.drag_to(&mut masks, (0.5, 0.8), aspect);
        let Shape::Radial { ry, .. } = masks.0[0].shape else {
            panic!()
        };
        assert!((ry - 0.3).abs() < 1e-9);
        tool.release();
        // Inside, away from the handles: the whole ellipse moves by the drag.
        assert!(tool.press(&masks, (0.45, 0.4), aspect, 0.02));
        assert!(matches!(tool.grab, Some((0, Grab::RadialBody(_)))));
        tool.drag_to(&mut masks, (0.55, 0.45), aspect);
        let Shape::Radial { cx, cy, .. } = masks.0[0].shape else {
            panic!()
        };
        assert!(
            (cx - 0.6).abs() < 1e-9 && (cy - 0.55).abs() < 1e-9,
            "{cx} {cy}"
        );
        tool.release();
        // Far outside catches nothing.
        assert!(!tool.press(&masks, (0.02, 0.02), aspect, 0.02));
    }

    #[test]
    fn a_linear_gradient_moves_each_end_or_the_whole() {
        let aspect = 1.0;
        let mut tool = MaskTool::default();
        let mut masks = Masks(vec![Mask::linear()]);
        // Start at (0.5, 0.15), end at (0.5, 0.5).
        assert!(tool.press(&masks, (0.5, 0.16), aspect, 0.02));
        assert_eq!(tool.grab, Some((0, Grab::LinearStart)));
        tool.drag_to(&mut masks, (0.3, 0.1), aspect);
        tool.release();
        assert!(tool.press(&masks, (0.5, 0.5), aspect, 0.02));
        assert_eq!(tool.grab, Some((0, Grab::LinearEnd)));
        tool.drag_to(&mut masks, (0.7, 0.6), aspect);
        tool.release();
        assert_eq!(
            masks.0[0].shape,
            Shape::Linear {
                x0: 0.3,
                y0: 0.1,
                x1: 0.7,
                y1: 0.6
            }
        );
        // The middle moves both ends together.
        assert!(tool.press(&masks, (0.5, 0.35), aspect, 0.02));
        tool.drag_to(&mut masks, (0.5, 0.45), aspect);
        let Shape::Linear { y0, y1, .. } = masks.0[0].shape else {
            panic!()
        };
        assert!((y0 - 0.2).abs() < 1e-9 && (y1 - 0.7).abs() < 1e-9);
        tool.release();
        // An end dragged onto the other is refused (no direction left).
        assert!(tool.press(&masks, (0.7, 0.7), aspect, 0.02));
        assert!(!tool.drag_to(&mut masks, (0.3, 0.2), aspect));
    }

    #[test]
    fn the_selected_mask_wins_where_two_overlap() {
        let aspect = 1.0;
        let mut tool = MaskTool::default();
        let masks = Masks(vec![radial(), radial()]);
        // Both centres are at (0.5, 0.5): the newest is caught first...
        assert!(tool.press(&masks, (0.5, 0.5), aspect, 0.02));
        assert_eq!(tool.selected, Some(1));
        // ...unless the other is selected.
        tool.release();
        tool.selected = Some(0);
        assert!(tool.press(&masks, (0.5, 0.5), aspect, 0.02));
        assert_eq!(tool.selected, Some(0));
    }

    #[test]
    fn outlines_and_lines_are_where_the_shape_is() {
        let aspect = 2.0;
        let shape = Mask::radial().shape;
        let ring = radial_outline(&shape, aspect, 4);
        // rx 0.3 of the height is 0.15 of a 2:1 photo's width.
        assert!((ring[0].0 - 0.65).abs() < 1e-9 && (ring[0].1 - 0.5).abs() < 1e-9);
        assert!((ring[1].1 - 0.7).abs() < 1e-9);
        let lines = linear_lines(&Mask::linear().shape, aspect, 0.1).unwrap();
        // The start line runs across (horizontally) through the start point.
        assert!((lines[0][0].1 - 0.15).abs() < 1e-9 && (lines[0][1].1 - 0.15).abs() < 1e-9);
        assert!(lines[0][0].0 != lines[0][1].0);
    }

    fn strokes(masks: &Masks, i: usize) -> &[Stroke] {
        match &masks.0[i].shape {
            Shape::Brush { strokes } => strokes,
            _ => panic!("a brush"),
        }
    }

    #[test]
    fn a_picked_brush_is_painted_and_points_are_thinned() {
        let aspect = 1.5;
        let mut tool = MaskTool::default();
        let mut masks = Masks::default();
        tool.add(&mut masks, Mask::radial()).unwrap();
        // A gradient picked: nothing to paint.
        assert!(!tool.start_stroke(&mut masks, (0.5, 0.5), false));
        tool.add(&mut masks, Mask::brush()).unwrap();
        assert_eq!(tool.brush_selected(&masks), Some(1));
        assert!(tool.start_stroke(&mut masks, (0.123456, 0.5), false));
        // Too close to the last point (a fifth of the 0.05 size): not kept.
        assert!(!tool.extend_stroke(&mut masks, (0.124, 0.5), aspect));
        assert!(tool.extend_stroke(&mut masks, (0.2, 0.5), aspect));
        assert!(tool.release());
        assert!(
            !tool.extend_stroke(&mut masks, (0.4, 0.5), aspect),
            "released"
        );
        let s = &strokes(&masks, 1)[0];
        assert_eq!(s.points, vec![[0.1235, 0.5], [0.2, 0.5]]);
        assert_eq!((s.size, s.erase), (0.05, false));
        // Alt erases; with the panel's Erase on, Alt paints.
        assert!(tool.start_stroke(&mut masks, (0.3, 0.3), true));
        tool.release();
        tool.brush.erase = true;
        assert!(tool.start_stroke(&mut masks, (0.3, 0.3), true));
        tool.release();
        let erased: Vec<bool> = strokes(&masks, 1).iter().map(|s| s.erase).collect();
        assert_eq!(erased, [false, true, false]);
        // The radial is untouched.
        assert_eq!(masks.0[0], Mask::radial());
    }
}
