//! The tools over the preview: crop (`R`) and spots (`Q`). Their overlays are drawn with
//! egui's painter over the preview texture; their maths lives in [`crop`] and [`spots`].
//!
//! Keys (the keyboard map in `shortcuts.rs`, read by the app): `R` and `Q` open or leave a tool, `Esc` leaves it (the
//! crop tool puts back the settings it was entered with), `Enter` applies the crop, `Delete`
//! removes the selected spot.

pub mod crop;
pub mod spots;

use super::Develop;
use super::panels;
use super::view::Viewport;
use crate::shortcuts::Action;
use crate::theme;
use crate::toast::{Level, Toasts};
use crop::CropTool;
use eframe::egui::{self, Color32, CursorIcon, Pos2, Rect, RichText, Stroke};
use lf_pipeline::renderer::View;
use lf_settings::geometry::{self, Geometry, apply, invert, output_to_source};
use lf_settings::{Crop, EditSettings, MAX_SPOTS, Spot, SpotMode};
use spots::SpotTool;

/// What a tool did: the settings changed (redraw), and/or a step must be committed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ToolOutput {
    pub changed: bool,
    pub commit: bool,
}

impl ToolOutput {
    pub fn or(self, other: ToolOutput) -> ToolOutput {
        ToolOutput {
            changed: self.changed || other.changed,
            commit: self.commit || other.commit,
        }
    }
}

/// Which tool is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Crop,
    Spots,
}

pub const CROP: &str = "Crop";
pub const SPOTS: &str = "Spots";
/// The crop panel's buttons: apply (`Enter`) and put back (`Esc`).
pub const DONE: &str = "Done";
pub const CANCEL: &str = "Cancel";

/// The open tool, if any.
#[derive(Debug, Clone)]
pub enum Active {
    Crop(Box<CropTool>),
    Spots(SpotTool),
}

/// The tools' state in the Develop view.
#[derive(Debug, Clone)]
pub struct Tools {
    pub active: Option<Active>,
    /// The mode new spots get, kept between visits to the spot tool.
    spot_mode: SpotMode,
}

impl Default for Tools {
    fn default() -> Tools {
        Tools {
            active: None,
            spot_mode: SpotMode::Heal,
        }
    }
}

impl Tools {
    pub fn kind(&self) -> Option<ToolKind> {
        match self.active {
            Some(Active::Crop(_)) => Some(ToolKind::Crop),
            Some(Active::Spots(_)) => Some(ToolKind::Spots),
            None => None,
        }
    }

    /// The spots changed under the tool (undo, redo, reset): nothing stays picked.
    pub fn clear_selection(&mut self) {
        if let Some(Active::Spots(tool)) = &mut self.active {
            tool.clear_selection();
        }
    }
}

/// Screen points ⇄ output fractions for the photo drawn into `shown` showing `view`.
#[derive(Debug, Clone, Copy)]
pub struct ScreenMap {
    pub shown: Rect,
    pub view: View,
}

impl ScreenMap {
    pub fn to_output(&self, p: Pos2) -> (f64, f64) {
        (
            self.view.x + f64::from((p.x - self.shown.left()) / self.shown.width()) * self.view.w,
            self.view.y + f64::from((p.y - self.shown.top()) / self.shown.height()) * self.view.h,
        )
    }

    pub fn to_screen(&self, (u, v): (f64, f64)) -> Pos2 {
        egui::pos2(
            self.shown.left() + ((u - self.view.x) / self.view.w) as f32 * self.shown.width(),
            self.shown.top() + ((v - self.view.y) / self.view.h) as f32 * self.shown.height(),
        )
    }

    /// `points` screen points as output fractions, per axis.
    fn span(&self, points: f32) -> (f64, f64) {
        (
            f64::from(points / self.shown.width()) * self.view.w,
            f64::from(points / self.shown.height()) * self.view.h,
        )
    }
}

/// Where the photo lands on screen (points) for a placement `rect` in physical pixels inside
/// the preview area `area`.
pub fn shown_rect(area: Rect, rect: (f64, f64, f64, f64), ppp: f32) -> Rect {
    let ppp = f64::from(ppp);
    let (rx, ry, rw, rh) = rect;
    Rect::from_min_size(
        area.min + egui::vec2((rx / ppp) as f32, (ry / ppp) as f32),
        egui::vec2((rw / ppp) as f32, (rh / ppp) as f32),
    )
}

/// Source UV ⇄ screen for the spot tool: the output map composed with the photo's geometry.
struct SpotMap {
    screen: ScreenMap,
    to_source: geometry::Affine,
    to_output: geometry::Affine,
    /// Screen points per photo width (a spot's radius → screen).
    scale: f64,
    aspect: f64,
}

impl SpotMap {
    fn new(screen: ScreenMap, s: &EditSettings, full: (u32, u32), output_w: f64) -> SpotMap {
        let to_source = output_to_source(full.0, full.1, &Geometry::from(s), &s.crop);
        SpotMap {
            screen,
            to_output: invert(&to_source),
            to_source,
            scale: f64::from(full.0) * f64::from(screen.shown.width())
                / (screen.view.w * output_w.max(1.0)),
            aspect: f64::from(full.0) / f64::from(full.1.max(1)),
        }
    }

    fn uv(&self, p: Pos2) -> (f64, f64) {
        let (u, v) = self.screen.to_output(p);
        apply(&self.to_source, u, v)
    }

    fn screen(&self, (x, y): (f64, f64)) -> Pos2 {
        self.screen.to_screen(apply(&self.to_output, x, y))
    }

    /// `points` on screen as a fraction of the photo's width.
    fn width_fraction(&self, points: f32) -> f64 {
        f64::from(points) / self.scale.max(1e-9)
    }
}

/// What the tool took from the pointer this frame (the view's pan and zoom leave it alone).
#[derive(Debug, Clone, Copy, Default)]
pub struct ToolInput {
    /// Drags belong to the tool, not to panning.
    pub owns_drag: bool,
    /// The wheel resized a spot instead of zooming.
    pub took_wheel: bool,
}

/// How near a crop handle the pointer must be (points).
const HANDLE_REACH: f32 = 10.0;
/// The smallest spot circle drawn and grabbed (points).
const MIN_SPOT_POINTS: f32 = 6.0;

/// A button in the crop panel.
enum CropAction {
    Done,
    Cancel,
}

impl Develop {
    /// The open tool.
    pub fn tool(&self) -> Option<ToolKind> {
        self.tools.kind()
    }

    /// The crop the preview draws instead of the settings' (the whole turned frame while
    /// cropping).
    pub(super) fn draw_crop(&self) -> Option<Crop> {
        matches!(self.tools.active, Some(Active::Crop(_))).then_some(crop::WHOLE)
    }

    /// The open photo's full size.
    fn full_size(&self) -> Option<(u32, u32)> {
        self.opened()
            .map(|o| (o.renderer.full_width, o.renderer.full_height))
    }

    fn absorb_tool(&mut self, out: ToolOutput, toasts: &mut Toasts) {
        if out.changed {
            self.settings_changed();
        }
        if out.commit {
            self.commit(toasts);
        }
    }

    /// Opens `kind`, or leaves it when it is already open.
    pub fn toggle_tool(&mut self, kind: ToolKind, toasts: &mut Toasts) {
        let was = self.tool();
        self.leave_tool(toasts);
        if was == Some(kind) || !self.is_ready() {
            return;
        }
        // A gesture in progress is its own step, before the tool's.
        self.commit(toasts);
        let Some(edit) = self.session.edit() else {
            return;
        };
        self.tools.active = Some(match kind {
            ToolKind::Crop => {
                // The output becomes the whole turned frame: start from fit.
                self.viewport = Viewport::default();
                Active::Crop(Box::new(CropTool::enter(&edit.settings)))
            }
            ToolKind::Spots => Active::Spots(SpotTool::new(self.tools.spot_mode)),
        });
        self.panels.curve.clear_selection();
        self.settings_changed();
    }

    /// Leaves the open tool: the crop session (or a spot resize still waiting) is committed.
    pub fn leave_tool(&mut self, toasts: &mut Toasts) {
        match self.tools.active.take() {
            Some(Active::Crop(tool)) => {
                self.viewport = Viewport::default();
                self.absorb_tool(tool.leave(), toasts);
                self.settings_changed();
            }
            Some(Active::Spots(tool)) => {
                self.tools.spot_mode = tool.mode;
                self.commit(toasts);
            }
            None => {}
        }
    }

    /// `Esc`: the crop tool puts back the settings it was entered with; the spot tool is left.
    pub fn cancel_tool(&mut self, toasts: &mut Toasts) {
        if let (Some(Active::Crop(tool)), Some(edit)) =
            (&self.tools.active, self.session.edit_mut())
        {
            let out = tool.cancel(edit);
            self.tools.active = None;
            self.viewport = Viewport::default();
            self.absorb_tool(out, toasts);
        } else {
            self.leave_tool(toasts);
        }
    }

    /// Whether a tool key's action is possible now: `R` and `Q` with a photo ready, `Esc` with
    /// a tool open, `Enter` while cropping, `Delete` with a spot selected.
    pub fn tool_enabled(&self, action: Action) -> bool {
        if !self.is_ready() {
            return false;
        }
        match action {
            Action::CropTool | Action::SpotTool => true,
            Action::LeaveTool => self.tool().is_some(),
            Action::ApplyCrop => self.tool() == Some(ToolKind::Crop),
            Action::DeleteSpot => {
                matches!(&self.tools.active, Some(Active::Spots(t)) if t.selected.is_some())
            }
            _ => false,
        }
    }

    /// `R`, `Q`, `Esc`, `Enter` and `Delete` (from the keyboard map; see [`Self::tool_enabled`]).
    pub fn tool_command(&mut self, action: Action, toasts: &mut Toasts) {
        match action {
            Action::CropTool => self.toggle_tool(ToolKind::Crop, toasts),
            Action::SpotTool => self.toggle_tool(ToolKind::Spots, toasts),
            Action::LeaveTool => self.cancel_tool(toasts),
            Action::ApplyCrop => self.leave_tool(toasts),
            Action::DeleteSpot => {
                if let (Some(Active::Spots(tool)), Some(edit)) =
                    (&mut self.tools.active, self.session.edit_mut())
                {
                    let mut out = ToolOutput::default();
                    edit.set(|s| out = tool.remove_selected(&mut s.spots));
                    self.absorb_tool(out, toasts);
                }
            }
            _ => {}
        }
    }

    /// The open tool's pointer handling over the photo drawn into `map` (`output_w`: the
    /// output's full-resolution width).
    pub(super) fn tool_input(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        map: ScreenMap,
        output_w: f64,
        toasts: &mut Toasts,
    ) -> ToolInput {
        let Some(full) = self.full_size() else {
            return ToolInput::default();
        };
        let (Some(active), Some(edit)) = (self.tools.active.as_mut(), self.session.edit_mut())
        else {
            return ToolInput::default();
        };
        let pointer = response.interact_pointer_pos();
        let origin = ui.input(|i| i.pointer.press_origin());
        let mut out = ToolOutput::default();
        let mut input = ToolInput {
            owns_drag: true,
            took_wheel: false,
        };
        match active {
            Active::Crop(tool) => {
                let (tw, th) = crop::turned(&edit.settings, full.0, full.1);
                let reach = map.span(HANDLE_REACH);
                if response.drag_started()
                    && let Some(at) = origin
                {
                    tool.press(&edit.settings, map.to_output(at), reach);
                }
                if response.dragged()
                    && tool.dragging()
                    && let Some(at) = pointer
                {
                    let p = map.to_output(at);
                    edit.set(|s| out.changed = tool.drag_to(s, p, tw, th));
                }
                if response.drag_stopped() {
                    out = out.or(tool.release());
                }
                if !ui.input(|i| i.pointer.any_down()) {
                    tool.straightening = false;
                }
                let handle = tool.handle().or_else(|| {
                    response.hover_pos().and_then(|at| {
                        crop::handle_at(&edit.settings.crop, map.to_output(at), reach)
                    })
                });
                if let Some(handle) = handle {
                    ui.ctx().set_cursor_icon(cursor(handle));
                }
            }
            Active::Spots(tool) => {
                let spot_map = SpotMap::new(map, &edit.settings, full, output_w);
                let aspect = spot_map.aspect;
                let min_r = spot_map.width_fraction(MIN_SPOT_POINTS);
                let mut press = |tool: &mut SpotTool, at: Pos2, radius: f64| {
                    let p = spot_map.uv(at);
                    let mut result = Ok(ToolOutput::default());
                    edit.set(|s| result = tool.press(&mut s.spots, p, aspect, min_r, radius));
                    match result {
                        Ok(o) => o,
                        Err(message) => {
                            toasts.push(Level::Warning, message);
                            ToolOutput::default()
                        }
                    }
                };
                if response.clicked()
                    && let Some(at) = origin.or(pointer)
                {
                    out = out.or(press(tool, at, spots::CLICK_RADIUS));
                    out = out.or(tool.release());
                }
                if response.drag_started()
                    && let Some(at) = origin
                {
                    out = out.or(press(tool, at, spots::MIN_RADIUS));
                }
                if response.dragged()
                    && let Some(at) = pointer
                {
                    let p = spot_map.uv(at);
                    edit.set(|s| out.changed |= tool.drag_to(&mut s.spots, p, aspect));
                }
                if response.drag_stopped() {
                    out = out.or(tool.release());
                }
                let now = ui.input(|i| i.time);
                if let Some(at) = response.hover_pos()
                    && let Some((index, _)) =
                        spots::hit_within(&edit.settings.spots, spot_map.uv(at), aspect, min_r)
                {
                    input.took_wheel = true;
                    let scroll = ui.input(|i| i.smooth_scroll_delta().y);
                    if scroll != 0.0 {
                        let factor = (f64::from(scroll) * 0.0015).exp();
                        edit.set(|s| out.changed |= tool.wheel(&mut s.spots, index, factor, now));
                    }
                } else if response.hovered() {
                    ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                }
                if tool.wheel_rested(now) {
                    out.commit = true;
                } else if let Some(at) = tool.resized_at {
                    let wait = spots::WHEEL_REST - (now - at);
                    ui.ctx().request_repaint_after_secs(wait as f32 + 0.02);
                }
            }
        }
        self.absorb_tool(out, toasts);
        input
    }

    /// Draws the open tool's overlay over the photo in `map`, clipped to `area`.
    pub(super) fn tool_paint(&self, ui: &egui::Ui, area: Rect, map: ScreenMap, output_w: f64) {
        let (Some(active), Some(edit), Some(full)) =
            (&self.tools.active, self.session.edit(), self.full_size())
        else {
            return;
        };
        let painter = ui.painter().with_clip_rect(area);
        match active {
            Active::Crop(tool) => paint_crop(&painter, map, &edit.settings.crop, tool.grid()),
            Active::Spots(tool) => {
                let spot_map = SpotMap::new(map, &edit.settings, full, output_w);
                for (i, s) in edit.settings.spots.iter().enumerate() {
                    paint_spot(&painter, &spot_map, s, tool.selected == Some(i));
                }
            }
        }
    }

    /// The tool buttons and the open tool's panel (above the adjust panels).
    pub(super) fn tool_ui(&mut self, ui: &mut egui::Ui, toasts: &mut Toasts) {
        let open = self.tool();
        let mut toggle = None;
        ui.horizontal(|ui| {
            let crop = ui
                .selectable_label(open == Some(ToolKind::Crop), CROP)
                .on_hover_text("Crop and straighten (R)");
            if crop.clicked() {
                toggle = Some(ToolKind::Crop);
            }
            let spot = ui
                .selectable_label(open == Some(ToolKind::Spots), SPOTS)
                .on_hover_text("Remove spots (Q)");
            if spot.clicked() {
                toggle = Some(ToolKind::Spots);
            }
        });
        if let Some(kind) = toggle {
            self.toggle_tool(kind, toasts);
        }
        let Some(full) = self.full_size() else {
            return;
        };
        let (Some(active), Some(edit)) = (self.tools.active.as_mut(), self.session.edit_mut())
        else {
            return;
        };
        let mut out = ToolOutput::default();
        let mut action = None;
        ui.add_space(4.0);
        match active {
            Active::Crop(tool) => {
                let mut p = panels::crop::CropPanelOutput::default();
                edit.set(|s| p = panels::crop::show(ui, s, full.0, full.1));
                if let Some(on) = p.straightening {
                    tool.straightening = on;
                }
                out.changed = p.changed;
                out.commit = p.commit;
                ui.horizontal(|ui| {
                    if ui.button(DONE).on_hover_text("Enter").clicked() {
                        action = Some(CropAction::Done);
                    }
                    if ui.button(CANCEL).on_hover_text("Esc").clicked() {
                        action = Some(CropAction::Cancel);
                    }
                });
            }
            Active::Spots(tool) => {
                edit.set(|s| out = spots_panel(ui, tool, &mut s.spots));
            }
        }
        ui.separator();
        self.absorb_tool(out, toasts);
        match action {
            Some(CropAction::Done) => self.leave_tool(toasts),
            Some(CropAction::Cancel) => self.cancel_tool(toasts),
            None => {}
        }
    }
}

/// The spot tool's panel: the hint, Heal / Clone, the count and "Remove spot".
fn spots_panel(ui: &mut egui::Ui, tool: &mut SpotTool, spots: &mut Vec<Spot>) -> ToolOutput {
    let mut out = ToolOutput::default();
    ui.label(RichText::new(spots::HINT).color(theme::MUTED));
    ui.horizontal(|ui| {
        ui.label("Mode");
        for (mode, label) in [
            (SpotMode::Heal, spots::HEAL),
            (SpotMode::Clone, spots::CLONE),
        ] {
            if ui.selectable_label(tool.mode == mode, label).clicked() {
                out = out.or(tool.set_mode(spots, mode));
            }
        }
    });
    ui.label(RichText::new(format!("{} of {MAX_SPOTS} spots", spots.len())).color(theme::MUTED));
    if spots.len() >= MAX_SPOTS {
        ui.label(RichText::new(spots::FULL).color(theme::WARNING));
    }
    if ui
        .add_enabled(tool.selected.is_some(), egui::Button::new(spots::REMOVE))
        .on_hover_text("Delete")
        .clicked()
    {
        out = out.or(tool.remove_selected(spots));
    }
    out
}

fn cursor(handle: crop::Handle) -> CursorIcon {
    use crop::Handle;
    match handle {
        Handle::Move => CursorIcon::Move,
        Handle::N | Handle::S => CursorIcon::ResizeVertical,
        Handle::E | Handle::W => CursorIcon::ResizeHorizontal,
        Handle::NW | Handle::SE => CursorIcon::ResizeNwSe,
        Handle::NE | Handle::SW => CursorIcon::ResizeNeSw,
    }
}

/// One spot: the line to its source, the source dashed, the spot solid (brand when picked).
fn paint_spot(painter: &egui::Painter, map: &SpotMap, s: &Spot, selected: bool) {
    let target = map.screen((s.x, s.y));
    let source = map.screen((s.sx, s.sy));
    let r = ((s.r * map.scale) as f32).max(MIN_SPOT_POINTS);
    painter.line_segment(
        [target, source],
        Stroke::new(1.0, Color32::from_white_alpha(150)),
    );
    let ring: Vec<Pos2> = (0..=48)
        .map(|k| {
            let a = k as f32 / 48.0 * std::f32::consts::TAU;
            source + egui::vec2(a.cos(), a.sin()) * r
        })
        .collect();
    let alpha = if selected { 255 } else { 180 };
    painter.extend(egui::Shape::dashed_line(
        &ring,
        Stroke::new(1.5, Color32::from_white_alpha(alpha)),
        4.0,
        3.0,
    ));
    painter.circle_stroke(
        target,
        r + 1.5,
        Stroke::new(1.0, Color32::from_black_alpha(90)),
    );
    let colour = if selected {
        theme::BRAND
    } else {
        Color32::WHITE
    };
    painter.circle_stroke(target, r, Stroke::new(2.0, colour));
}

/// The crop frame over the whole turned photo: dimmed outside, the frame, the handles, and
/// the thirds while dragging.
fn paint_crop(painter: &egui::Painter, map: ScreenMap, c: &Crop, grid: bool) {
    let frame = Rect::from_min_max(
        map.to_screen((c.x, c.y)),
        map.to_screen((c.x + c.w, c.y + c.h)),
    );
    let photo = map.shown;
    let dim = Color32::from_black_alpha(140);
    for r in [
        Rect::from_min_max(photo.min, egui::pos2(photo.max.x, frame.min.y)),
        Rect::from_min_max(egui::pos2(photo.min.x, frame.max.y), photo.max),
        Rect::from_min_max(
            egui::pos2(photo.min.x, frame.min.y),
            egui::pos2(frame.min.x, frame.max.y),
        ),
        Rect::from_min_max(
            egui::pos2(frame.max.x, frame.min.y),
            egui::pos2(photo.max.x, frame.max.y),
        ),
    ] {
        if r.is_positive() {
            painter.rect_filled(r, 0.0, dim);
        }
    }
    if grid {
        let line = Stroke::new(1.0, Color32::from_white_alpha(110));
        for k in [1.0 / 3.0, 2.0 / 3.0] {
            let x = frame.left() + frame.width() * k;
            let y = frame.top() + frame.height() * k;
            painter.line_segment(
                [egui::pos2(x, frame.top()), egui::pos2(x, frame.bottom())],
                line,
            );
            painter.line_segment(
                [egui::pos2(frame.left(), y), egui::pos2(frame.right(), y)],
                line,
            );
        }
    }
    painter.rect_stroke(
        frame,
        0.0,
        Stroke::new(1.0, Color32::from_white_alpha(230)),
        egui::StrokeKind::Middle,
    );
    for handle in crop::Handle::RESIZE {
        let (ax, ay) = handle.anchor();
        let at = egui::pos2(
            frame.left() + frame.width() * ax as f32,
            frame.top() + frame.height() * ay as f32,
        );
        let size = match handle {
            crop::Handle::N | crop::Handle::S => egui::vec2(18.0, 6.0),
            crop::Handle::E | crop::Handle::W => egui::vec2(6.0, 18.0),
            _ => egui::vec2(10.0, 10.0),
        };
        let r = Rect::from_center_size(at, size);
        painter.rect_filled(r, 2.0, Color32::from_black_alpha(80));
        painter.rect_stroke(
            r,
            2.0,
            Stroke::new(2.0, Color32::WHITE),
            egui::StrokeKind::Middle,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_map_round_trips_through_a_zoomed_view() {
        let map = ScreenMap {
            shown: Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(800.0, 600.0)),
            view: View {
                x: 0.25,
                y: 0.5,
                w: 0.5,
                h: 0.25,
            },
        };
        let (u, v) = map.to_output(egui::pos2(500.0, 350.0));
        assert!(
            (u - 0.5).abs() < 1e-6 && (v - 0.625).abs() < 1e-6,
            "{u} {v}"
        );
        let back = map.to_screen((u, v));
        assert!((back.x - 500.0).abs() < 1e-3 && (back.y - 350.0).abs() < 1e-3);
    }

    #[test]
    fn spot_map_follows_turns_and_measures_radius_in_width() {
        // A 3000×2000 photo turned a quarter: the output is 2000 wide, shown 1000 points
        // wide, so one source pixel is half a point and a spot of r = 0.1 (300 px) is 150.
        let mut s = EditSettings::default_settings();
        s.rotate = lf_settings::Rotate::R90;
        let screen = ScreenMap {
            shown: Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 1500.0)),
            view: View::default(),
        };
        let m = SpotMap::new(screen, &s, (3000, 2000), 2000.0);
        assert!((m.scale * 0.1 - 150.0).abs() < 1e-6, "{}", m.scale);
        let uv = m.uv(egui::pos2(250.0, 300.0));
        let back = m.screen(uv);
        assert!((back.x - 250.0).abs() < 1e-3 && (back.y - 300.0).abs() < 1e-3);
        // The source's top left lands at the turned frame's top right.
        let corner = m.screen((0.0, 0.0));
        assert!(
            (corner.x - 1000.0).abs() < 1e-3 && corner.y.abs() < 1e-3,
            "{corner:?}"
        );
    }
}
