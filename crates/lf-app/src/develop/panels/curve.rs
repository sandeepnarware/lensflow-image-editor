//! The tone curve editor, after the web's `CurveEditor.vue`: RGB, R, G and B tabs over a
//! 256-point square. Drag a point to move it, click to add one, drag it off the box (or
//! double-click it, or press `Delete`) to remove it; the two end points stay. Points keep
//! their x strictly increasing ([`clamp_point`]). The line is the curve's lookup table from
//! `lf_settings::curve::curve_table`, the photo's luma histogram behind it.

use crate::theme;
use eframe::egui::{self, Color32, Key, Modifiers, Pos2, Rect, Sense, Shape, Stroke, pos2, vec2};
use lf_pipeline::renderer::Histogram;
use lf_settings::curve::curve_table;
use lf_settings::{CurvePoint, Curves};

/// The editor's side in points.
pub const SIZE: f32 = 256.0;
/// The closest two points may come in x.
pub const GAP: f64 = 0.01;
/// The most points a curve holds (as the web).
pub const MAX_POINTS: usize = 16;
/// How near (in points) a press must be to grab a point.
const GRAB: f32 = 14.0;
/// How far outside the box (in points) a dragged point is removed.
const DROP_OFF: f32 = 30.0;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Channel {
    #[default]
    Rgb,
    R,
    G,
    B,
}

impl Channel {
    pub const ALL: [Channel; 4] = [Channel::Rgb, Channel::R, Channel::G, Channel::B];

    pub fn label(self) -> &'static str {
        match self {
            Channel::Rgb => "RGB",
            Channel::R => "R",
            Channel::G => "G",
            Channel::B => "B",
        }
    }

    fn colour(self) -> Color32 {
        match self {
            Channel::Rgb => theme::INK,
            Channel::R => Color32::from_rgb(0xef, 0x44, 0x44),
            Channel::G => Color32::from_rgb(0x22, 0xc5, 0x5e),
            Channel::B => Color32::from_rgb(0x3b, 0x82, 0xf6),
        }
    }

    pub fn points(self, curves: &mut Curves) -> &mut Vec<CurvePoint> {
        match self {
            Channel::Rgb => &mut curves.rgb,
            Channel::R => &mut curves.r,
            Channel::G => &mut curves.g,
            Channel::B => &mut curves.b,
        }
    }
}

/// Where point `i` may go when dragged to `p`: y inside 0..1; an end point keeps its x; an
/// inner point stays at least [`GAP`] from its neighbours, so x stays strictly increasing.
pub fn clamp_point(points: &[CurvePoint], i: usize, p: CurvePoint) -> CurvePoint {
    let y = p[1].clamp(0.0, 1.0);
    let last = points.len().saturating_sub(1);
    if i == 0 || i >= last {
        return [points.get(i).map_or(p[0], |q| q[0]), y];
    }
    let (lo, hi) = (points[i - 1][0] + GAP, points[i + 1][0] - GAP);
    [p[0].clamp(lo, hi.max(lo)), y]
}

/// Adds a point at `p` (kept in x order); its index, or `None` when the curve is full or `p`
/// is not strictly inside the end points with room either side.
pub fn add_point(points: &mut Vec<CurvePoint>, p: CurvePoint) -> Option<usize> {
    if points.len() >= MAX_POINTS {
        return None;
    }
    let at = points.iter().position(|q| q[0] > p[0])?;
    if at == 0 || p[0] - points[at - 1][0] < GAP || points[at][0] - p[0] < GAP {
        return None;
    }
    points.insert(at, [p[0], p[1].clamp(0.0, 1.0)]);
    Some(at)
}

/// Removes point `i` unless it is an end point.
pub fn remove_point(points: &mut Vec<CurvePoint>, i: usize) -> bool {
    if i == 0 || i + 1 >= points.len() {
        return false;
    }
    points.remove(i);
    true
}

/// The editor's own state between frames.
#[derive(Debug, Clone, Default)]
pub struct CurveState {
    pub channel: Channel,
    /// The point being dragged.
    dragging: Option<usize>,
    /// The point last touched (`Delete` removes it).
    selected: Option<usize>,
    /// Something changed during the current press.
    moved: bool,
}

/// What the editor did this frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CurveResponse {
    pub changed: bool,
    /// A point was added, moved, removed, or a channel reset: one history step.
    pub released: bool,
}

/// Draws the tabs and the editor for `curves`.
pub fn edit(
    ui: &mut egui::Ui,
    state: &mut CurveState,
    curves: &mut Curves,
    histogram: Option<&Histogram>,
) -> CurveResponse {
    let mut out = CurveResponse::default();
    ui.horizontal(|ui| {
        for channel in Channel::ALL {
            if ui
                .selectable_label(state.channel == channel, channel.label())
                .clicked()
            {
                state.channel = channel;
                state.dragging = None;
                state.selected = None;
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("Reset curve").clicked() {
                let points = state.channel.points(curves);
                let identity = lf_settings::identity_curve();
                if *points != identity {
                    *points = identity;
                    out.changed = true;
                    out.released = true;
                }
                state.selected = None;
            }
        });
    });

    let side = ui.available_width().min(SIZE);
    let (rect, response) = ui.allocate_exact_size(vec2(side, side), Sense::click_and_drag());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::Role::Image,
            true,
            "Tone curve: click to add a point, drag to move, double-click to remove",
        )
    });
    let channel = state.channel;
    let points = channel.points(curves);
    let to_curve = |p: Pos2| -> CurvePoint {
        [
            f64::from((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0),
            f64::from(1.0 - (p.y - rect.top()) / rect.height()).clamp(0.0, 1.0),
        ]
    };
    let to_screen = |p: &CurvePoint| -> Pos2 {
        pos2(
            rect.left() + rect.width() * p[0] as f32,
            rect.top() + rect.height() * (1.0 - p[1] as f32),
        )
    };
    let near = |points: &[CurvePoint], at: Pos2| -> Option<usize> {
        points
            .iter()
            .enumerate()
            .map(|(i, p)| (i, to_screen(p).distance(at)))
            .filter(|(_, d)| *d < GRAB)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    };

    let pointer = response.interact_pointer_pos();
    if response.double_clicked() {
        if let Some(i) = pointer.and_then(|at| near(points, at))
            && remove_point(points, i)
        {
            out.changed = true;
            out.released = true;
        }
        state.selected = None;
        state.dragging = None;
    } else if (response.drag_started() || (response.clicked() && state.dragging.is_none()))
        && let Some(at) = pointer
    {
        state.moved = false;
        match near(points, at) {
            Some(i) => state.dragging = Some(i),
            None => {
                state.dragging = add_point(points, to_curve(at));
                state.moved = state.dragging.is_some();
                out.changed |= state.moved;
            }
        }
        state.selected = state.dragging;
    }
    if response.dragged()
        && let (Some(i), Some(at)) = (state.dragging, pointer)
    {
        let outside = at.y < rect.top() - DROP_OFF || at.y > rect.bottom() + DROP_OFF;
        if outside {
            if remove_point(points, i) {
                state.moved = true;
                out.changed = true;
            }
            state.dragging = None;
            state.selected = None;
        } else if i < points.len() {
            let next = clamp_point(points, i, to_curve(at));
            if points[i] != next {
                points[i] = next;
                state.moved = true;
                out.changed = true;
            }
        }
    }
    // A plain click (added a point) ends at once; a drag ends on release.
    if response.drag_stopped() || (response.clicked() && !response.dragged()) {
        out.released |= state.moved;
        state.moved = false;
        state.dragging = None;
    }
    if let Some(i) = state.selected
        && !ui.ctx().egui_wants_keyboard_input()
        && ui.input_mut(|inp| inp.consume_key(Modifiers::NONE, Key::Delete))
    {
        if remove_point(points, i) {
            out.changed = true;
            out.released = true;
        }
        state.selected = None;
    }

    paint(ui, rect, points, channel, state.selected, histogram);
    out
}

fn paint(
    ui: &egui::Ui,
    rect: Rect,
    points: &[CurvePoint],
    channel: Channel,
    selected: Option<usize>,
    histogram: Option<&Histogram>,
) {
    let painter = ui.painter_at(rect.expand(8.0));
    painter.rect_filled(rect, 6.0, theme::SURFACE_2);
    if let Some(h) = histogram {
        let tallest = h.l[2..254].iter().copied().max().unwrap_or(1).max(1) as f32;
        let mut line = vec![pos2(rect.left(), rect.bottom())];
        for (i, n) in h.l.iter().enumerate() {
            let x = rect.left() + rect.width() * i as f32 / 255.0;
            let y = rect.bottom() - (*n as f32 / tallest).min(1.0) * rect.height() * 0.9;
            line.push(pos2(x, y));
        }
        line.push(pos2(rect.right(), rect.bottom()));
        let fill = theme::MUTED.gamma_multiply(0.2);
        for pair in line.windows(2) {
            let quad = vec![
                pair[0],
                pair[1],
                pos2(pair[1].x, rect.bottom()),
                pos2(pair[0].x, rect.bottom()),
            ];
            painter.add(Shape::convex_polygon(quad, fill, Stroke::NONE));
        }
    }
    let grid = Stroke::new(1.0, theme::LINE);
    for i in 1..4 {
        let t = i as f32 / 4.0;
        let x = rect.left() + rect.width() * t;
        let y = rect.top() + rect.height() * t;
        painter.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], grid);
        painter.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], grid);
    }
    painter.add(Shape::dashed_line(
        &[rect.left_bottom(), rect.right_top()],
        grid,
        3.0,
        4.0,
    ));
    let table = curve_table(points, 64);
    let line: Vec<Pos2> = table
        .iter()
        .enumerate()
        .map(|(i, y)| {
            pos2(
                rect.left() + rect.width() * i as f32 / 63.0,
                rect.top() + rect.height() * (1.0 - y),
            )
        })
        .collect();
    let colour = channel.colour();
    painter.add(Shape::line(line, Stroke::new(2.0, colour)));
    for (i, p) in points.iter().enumerate() {
        let at = pos2(
            rect.left() + rect.width() * p[0] as f32,
            rect.top() + rect.height() * (1.0 - p[1] as f32),
        );
        let ring = if selected == Some(i) {
            theme::BRAND
        } else {
            theme::SURFACE
        };
        painter.circle(at, 5.0, colour, Stroke::new(2.0, ring));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_clamp_keeps_x_monotonic_and_endpoints_fixed() {
        let points = vec![[0.0, 0.0], [0.3, 0.25], [0.6, 0.7], [1.0, 1.0]];
        // An inner point stops short of its neighbours.
        assert_eq!(clamp_point(&points, 1, [0.9, 0.5]), [0.59, 0.5]);
        assert_eq!(clamp_point(&points, 2, [0.1, 0.5]), [0.31, 0.5]);
        assert_eq!(
            clamp_point(&points, 1, [0.4, 1.7]),
            [0.4, 1.0],
            "y stays in 0..1"
        );
        // End points move up and down only.
        assert_eq!(clamp_point(&points, 0, [0.5, 0.2]), [0.0, 0.2]);
        assert_eq!(clamp_point(&points, 3, [0.2, -0.4]), [1.0, 0.0]);

        // x stays strictly increasing whatever the drags.
        let mut pts = points.clone();
        for (i, p) in [(1, [0.95, 0.1]), (2, [0.0, 0.9]), (1, [0.61, 0.3])] {
            let moved = clamp_point(&pts, i, p);
            pts[i] = moved;

            assert!(pts.windows(2).all(|w| w[0][0] < w[1][0]), "{pts:?}");
        }

        // Adding keeps order; end points cannot be removed; the curve holds 16 at most.
        let mut pts = lf_settings::identity_curve();
        assert_eq!(add_point(&mut pts, [0.5, 0.6]), Some(1));
        assert_eq!(add_point(&mut pts, [0.25, 0.2]), Some(1));
        assert_eq!(add_point(&mut pts, [0.255, 0.2]), None, "too close");
        assert_eq!(add_point(&mut pts, [0.0, 0.5]), None, "on an end point");
        let last = pts.len() - 1;
        assert!(!remove_point(&mut pts, 0) && !remove_point(&mut pts, last));
        assert!(remove_point(&mut pts, 1));
        while pts.len() < MAX_POINTS {
            let x = pts[pts.len() - 2][0] + 0.02;
            add_point(&mut pts, [x, x]).unwrap();
        }
        assert_eq!(add_point(&mut pts, [0.99, 0.5]), None, "full");
    }
}
