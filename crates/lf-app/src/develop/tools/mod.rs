//! The tools over the preview: crop (`R`) and spots (`Q`). Their overlays are drawn with
//! egui's painter over the preview texture; their maths lives in [`crop`] and [`spots`].
//!
//! Keys (the keyboard map in `shortcuts.rs`, read by the app): `R` and `Q` open or leave a tool, `Esc` leaves it (the
//! crop tool puts back the settings it was entered with), `Enter` applies the crop, `Delete`
//! removes the selected spot.

pub mod crop;
pub mod masks;
pub mod spots;

use super::panels;
use super::view::Viewport;
use super::{Develop, auto};
use crate::shortcuts::Action;
use crate::theme;
use crate::toast::{Level, Toasts};
use crop::CropTool;
use eframe::egui::{self, Color32, CursorIcon, Pos2, Rect, RichText, Stroke};
use lf_pipeline::renderer::View;
use lf_settings::geometry::{self, Geometry, apply, invert, output_to_source};
use lf_settings::heal::{HealStroke, Heals};
use lf_settings::lens::Lens;
use lf_settings::masks::{Mask, Masks, Range as MaskRange, Shape};
use lf_settings::{Crop, EditSettings, MAX_SPOTS, Spot, SpotMode};
use masks::MaskTool;
use panels::slider::Range;
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
    /// The white-balance eyedropper: one click sets temperature and tint, then it closes.
    WhiteBalance,
    /// Local adjustments: linear and radial gradients with their own sliders.
    Masks,
    /// Guided Upright: lines drawn along edges that should be upright or level.
    Guided,
}

pub const CROP: &str = "Crop";
pub const SPOTS: &str = "Spots";
pub const WHITE_BALANCE: &str = "White balance";
pub const MASKS: &str = "Masks";
pub const GUIDED: &str = "Guided";
/// Guided Upright's hint, in its panel.
pub const GUIDED_HINT: &str = "Drag along two edges that should be upright (and two that should be level, if you like). The photo follows each guide. Esc leaves.";
pub const CLEAR_GUIDES: &str = "Clear guides";
pub const GUIDED_TURNED: &str = "Guided Upright works on a photo that is not turned or flipped.";
/// The most guides kept (a fifth replaces the oldest), and the shortest kept (points).
pub const MAX_GUIDES: usize = 4;
const MIN_GUIDE_POINTS: f32 = 20.0;

/// Guided Upright's state: the guides drawn (in the photo as shot, so they stay on the
/// photo's edges as the correction changes) and the one being drawn.
#[derive(Debug, Clone, Default)]
pub struct GuidedTool {
    pub guides: Vec<lf_settings::upright::Segment>,
    drawing: Option<((f64, f64), (f64, f64))>,
}
/// The eyedropper's hint, in its panel.
pub const WB_HINT: &str = "Click something that should be grey or white. Esc leaves.";
/// The crop panel's buttons: apply (`Enter`) and put back (`Esc`).
pub const DONE: &str = "Done";
pub const CANCEL: &str = "Cancel";

/// The open tool, if any.
#[derive(Debug, Clone)]
pub enum Active {
    Crop(Box<CropTool>),
    Spots(SpotTool),
    WhiteBalance,
    Masks(MaskTool),
    Guided(GuidedTool),
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
            Some(Active::WhiteBalance) => Some(ToolKind::WhiteBalance),
            Some(Active::Masks(_)) => Some(ToolKind::Masks),
            Some(Active::Guided(_)) => Some(ToolKind::Guided),
            None => None,
        }
    }

    /// The spots changed under the tool (undo, redo, reset): nothing stays picked.
    pub fn clear_selection(&mut self) {
        if let Some(Active::Spots(tool)) = &mut self.active {
            tool.clear_selection();
        }
        if let Some(Active::Masks(tool)) = &mut self.active {
            tool.grab = None;
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
    /// Spots and the eyedropper work in the photo as shot: through the lens corrections
    /// when they move pixels (masks stay in the corrected photo, as the shader places them).
    lens: Option<Lens>,
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
            lens: None,
        }
    }

    /// The same map, ending in the photo as shot (for spots and the eyedropper).
    fn through_lens(mut self, s: &EditSettings) -> SpotMap {
        let lens = Lens::from_settings(s);
        self.lens = lens.moves_pixels().then_some(lens);
        self
    }

    fn uv(&self, p: Pos2) -> (f64, f64) {
        let (u, v) = self.screen.to_output(p);
        let corrected = apply(&self.to_source, u, v);
        match &self.lens {
            Some(lens) => lens.to_source(corrected, self.aspect),
            None => corrected,
        }
    }

    fn screen(&self, p: (f64, f64)) -> Pos2 {
        let (x, y) = match &self.lens {
            Some(lens) => lens.to_corrected(p, self.aspect),
            None => p,
        };
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
            ToolKind::WhiteBalance => Active::WhiteBalance,
            ToolKind::Masks => Active::Masks(MaskTool::default()),
            ToolKind::Guided => Active::Guided(GuidedTool::default()),
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
            Some(Active::Masks(_)) => self.commit(toasts),
            Some(Active::WhiteBalance) | Some(Active::Guided(_)) | None => {}
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
            Action::CropTool | Action::SpotTool | Action::WhiteBalanceTool | Action::MaskTool => {
                true
            }
            Action::LeaveTool => self.tool().is_some(),
            Action::ApplyCrop => self.tool() == Some(ToolKind::Crop),
            Action::DeleteSpot => {
                matches!(&self.tools.active, Some(Active::Spots(t)) if t.selected.is_some()
                    || (t.brush && t.heal_selected.is_some()))
                    || matches!(&self.tools.active, Some(Active::Masks(t)) if t.selected.is_some())
            }
            _ => false,
        }
    }

    /// `R`, `Q`, `Esc`, `Enter` and `Delete` (from the keyboard map; see [`Self::tool_enabled`]).
    pub fn tool_command(&mut self, action: Action, toasts: &mut Toasts) {
        match action {
            Action::CropTool => self.toggle_tool(ToolKind::Crop, toasts),
            Action::SpotTool => self.toggle_tool(ToolKind::Spots, toasts),
            Action::WhiteBalanceTool => self.toggle_tool(ToolKind::WhiteBalance, toasts),
            Action::MaskTool => self.toggle_tool(ToolKind::Masks, toasts),
            Action::LeaveTool => self.cancel_tool(toasts),
            Action::ApplyCrop => self.leave_tool(toasts),
            Action::DeleteSpot => {
                let mut out = ToolOutput::default();
                match (&mut self.tools.active, self.session.edit_mut()) {
                    (Some(Active::Spots(tool)), Some(edit)) if tool.brush => {
                        edit.set(|s| out = remove_stroke(tool, s));
                    }
                    (Some(Active::Spots(tool)), Some(edit)) => {
                        edit.set(|s| out = tool.remove_selected(&mut s.spots));
                    }
                    (Some(Active::Masks(tool)), Some(edit)) => {
                        edit.set(|s| {
                            let mut masks = Masks::from_settings(s);
                            if tool.remove_selected(&mut masks) {
                                masks.write_into(s);
                                out = ToolOutput {
                                    changed: true,
                                    commit: true,
                                };
                            }
                        });
                    }
                    _ => {}
                }
                self.absorb_tool(out, toasts);
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
        // Guided Upright: a guide finished this frame (fit once the borrows end).
        let mut guided = false;
        // Where the eyedropper was clicked (the sample is read once the borrows end).
        let mut picked = None;
        // Where Alt-click picked a mask's range colour (read the same way).
        let mut picked_colour = None;
        match active {
            Active::Masks(tool) => {
                let spot_map = SpotMap::new(map, &edit.settings, full, output_w);
                let aspect = spot_map.aspect;
                // The handles' reach, in the masks' square units (fractions of the height).
                let reach = spot_map.width_fraction(HANDLE_REACH) * aspect;
                let alt = ui.input(|i| i.modifiers.alt);
                // Alt-click on the photo picks the colour of a colour range.
                let picking_colour = alt
                    && tool
                        .selected
                        .and_then(|i| Masks::from_settings(&edit.settings).0.get(i).cloned())
                        .is_some_and(|m| matches!(m.range, MaskRange::Colour { .. }));
                if picking_colour
                    && response.clicked()
                    && let Some(at) = origin.or(pointer)
                {
                    picked_colour = Some(
                        SpotMap::new(map, &edit.settings, full, output_w)
                            .through_lens(&edit.settings)
                            .uv(at),
                    );
                }
                // Select object: the next click says which object (and does nothing else).
                let picking_object = tool.picking_object && !picking_colour;
                if picking_object {
                    if response.hovered() {
                        ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                    }
                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        tool.picking_object = false;
                    }
                    if response.clicked()
                        && let Some(at) = origin.or(pointer)
                    {
                        tool.object_request = Some(spot_map.uv(at));
                        tool.picking_object = false;
                    }
                    input.owns_drag = false;
                }
                let painting = !picking_colour
                    && !picking_object
                    && tool
                        .brush_selected(&Masks::from_settings(&edit.settings))
                        .is_some();
                // A picked brush mask is painted: a drag lays a stroke, a click a dab.
                let mut stroke_at = |tool: &mut MaskTool, at: Pos2, out: &mut ToolOutput| {
                    let p = spot_map.uv(at);
                    edit.set(|s| {
                        let mut masks = Masks::from_settings(s);
                        let changed = if tool.painting.is_some() {
                            tool.extend_stroke(&mut masks, p, aspect)
                        } else {
                            tool.start_stroke(&mut masks, p, alt)
                        };
                        if changed {
                            masks.write_into(s);
                            out.changed = true;
                        }
                    });
                };
                if painting {
                    if response.drag_started()
                        && let Some(at) = origin
                    {
                        stroke_at(tool, at, &mut out);
                    }
                    if response.dragged()
                        && tool.painting.is_some()
                        && let Some(at) = pointer
                    {
                        stroke_at(tool, at, &mut out);
                    }
                    if response.clicked()
                        && let Some(at) = origin.or(pointer)
                    {
                        stroke_at(tool, at, &mut out);
                    }
                    if (response.drag_stopped() || response.clicked()) && tool.release() {
                        out.commit = true;
                    }
                }
                if !painting
                    && !picking_object
                    && response.drag_started()
                    && let Some(at) = origin
                {
                    tool.press(
                        &Masks::from_settings(&edit.settings),
                        spot_map.uv(at),
                        aspect,
                        reach,
                    );
                }
                if response.dragged()
                    && tool.grab.is_some()
                    && let Some(at) = pointer
                {
                    let p = spot_map.uv(at);
                    edit.set(|s| {
                        let mut masks = Masks::from_settings(s);
                        if tool.drag_to(&mut masks, p, aspect) {
                            masks.write_into(s);
                            out.changed = true;
                        }
                    });
                }
                if response.drag_stopped() && tool.release() {
                    out.commit = true;
                }
                // A click picks the mask under it (and changes nothing).
                if !painting
                    && !picking_colour
                    && !picking_object
                    && response.clicked()
                    && let Some(at) = origin.or(pointer)
                {
                    tool.press(
                        &Masks::from_settings(&edit.settings),
                        spot_map.uv(at),
                        aspect,
                        reach,
                    );
                    tool.release();
                }
                // A drag that caught no handle (with no brush picked) pans the photo.
                input.owns_drag = !picking_object && (painting || tool.grab.is_some());
                if response.hovered() {
                    ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                }
            }
            Active::Guided(tool) => {
                let m =
                    SpotMap::new(map, &edit.settings, full, output_w).through_lens(&edit.settings);
                if response.hovered() {
                    ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                }
                if response.drag_started()
                    && let Some(at) = origin
                {
                    let p = m.uv(at);
                    tool.drawing = Some((p, p));
                }
                if response.dragged()
                    && let (Some((a, _)), Some(at)) = (tool.drawing, pointer)
                {
                    tool.drawing = Some((a, m.uv(at)));
                }
                if response.drag_stopped()
                    && let Some((a, b)) = tool.drawing.take()
                    && m.screen(a).distance(m.screen(b)) >= MIN_GUIDE_POINTS
                {
                    tool.guides
                        .push(lf_settings::upright::Segment { a, b, weight: 1.0 });
                    if tool.guides.len() > MAX_GUIDES {
                        tool.guides.remove(0);
                    }
                    guided = true;
                }
            }
            Active::WhiteBalance => {
                if response.hovered() {
                    ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                }
                if response.clicked()
                    && let Some(at) = origin.or(pointer)
                {
                    picked = Some(
                        SpotMap::new(map, &edit.settings, full, output_w)
                            .through_lens(&edit.settings)
                            .uv(at),
                    );
                }
                // A drag still pans the photo.
                input.owns_drag = false;
            }
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
            Active::Spots(tool) if tool.brush => {
                let spot_map =
                    SpotMap::new(map, &edit.settings, full, output_w).through_lens(&edit.settings);
                let aspect = spot_map.aspect;
                let heals = Heals::from_settings(&edit.settings);
                // A finished line becomes a stroke, its source beside it.
                let add = |edit: &mut crate::develop::edit_state::EditState,
                           tool: &mut SpotTool,
                           points: Vec<[f64; 2]>,
                           out: &mut ToolOutput,
                           toasts: &mut Toasts| {
                    let r = tool.brush_r;
                    let (dx, dy) = HealStroke::default_offset(&points, r, aspect);
                    let mode = tool.mode;
                    let mut full = false;
                    edit.set(|s| {
                        let mut h = Heals::from_settings(s);
                        if h.0.len() >= lf_settings::heal::MAX_STROKES {
                            full = true;
                            return;
                        }
                        h.0.push(HealStroke {
                            points,
                            r,
                            dx,
                            dy,
                            mode,
                        });
                        tool.heal_selected = Some(h.0.len() - 1);
                        h.write_into(s);
                        out.changed = true;
                        out.commit = true;
                    });
                    if full {
                        toasts.push(Level::Warning, spots::STROKES_FULL);
                    }
                };
                if response.drag_started()
                    && let Some(at) = origin
                {
                    let p = spot_map.uv(at);
                    // Grabbing the picked stroke's source line moves where it copies from.
                    let grab = tool
                        .heal_selected
                        .and_then(|i| heals.0.get(i).map(|s| (i, s)))
                        .filter(|(_, s)| {
                            let source: Vec<[f64; 2]> = s
                                .points
                                .iter()
                                .map(|q| [q[0] + s.dx, q[1] + s.dy])
                                .collect();
                            spots::stroke_distance(&source, p, aspect) <= s.r
                        });
                    match grab {
                        Some((i, s)) => tool.heal_grab = Some((i, (s.dx, s.dy), p)),
                        None => tool.stroke = Some(vec![[p.0, p.1]]),
                    }
                }
                if response.dragged()
                    && let Some(at) = pointer
                {
                    let p = spot_map.uv(at);
                    if let Some((i, (dx0, dy0), start)) = tool.heal_grab {
                        edit.set(|s| {
                            let mut h = Heals::from_settings(s);
                            if let Some(st) = h.0.get_mut(i) {
                                st.dx = dx0 + p.0 - start.0;
                                st.dy = dy0 + p.1 - start.1;
                                h.write_into(s);
                                out.changed = true;
                            }
                        });
                    } else if let Some(points) = &mut tool.stroke
                        && let Some(last) = points.last().copied()
                        && (p.0 - last[0]).hypot((p.1 - last[1]) / aspect) >= tool.brush_r * 0.25
                        && points.len() < lf_settings::heal::MAX_POINTS
                    {
                        points.push([p.0, p.1]);
                    }
                }
                if response.drag_stopped() {
                    if tool.heal_grab.take().is_some() {
                        out.commit = true;
                    }
                    if let Some(points) = tool.stroke.take() {
                        add(edit, tool, points, &mut out, toasts);
                    }
                }
                if response.clicked()
                    && let Some(at) = origin.or(pointer)
                {
                    let p = spot_map.uv(at);
                    // A click on a stroke picks it; elsewhere it heals a dab there.
                    let hit = heals
                        .0
                        .iter()
                        .position(|s| spots::stroke_distance(&s.points, p, aspect) <= s.r);
                    match hit {
                        Some(i) => tool.heal_selected = Some(i),
                        None => add(edit, tool, vec![[p.0, p.1]], &mut out, toasts),
                    }
                }
                if response.hovered() {
                    ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                }
            }
            Active::Spots(tool) => {
                let spot_map =
                    SpotMap::new(map, &edit.settings, full, output_w).through_lens(&edit.settings);
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
        if guided {
            self.apply_guides(full, toasts);
        }
        if let Some(at) = picked {
            self.pick_white_balance(at, toasts);
        }
        if let Some(at) = picked_colour {
            self.pick_range_colour(at, toasts);
        }
        input
    }

    /// Guided Upright: the perspective and straightening that make the guides upright and
    /// level (once there are two of a kind), one history step. The crop is kept inside, as
    /// Upright's buttons keep it.
    fn apply_guides(&mut self, full: (u32, u32), toasts: &mut Toasts) {
        let Some(Active::Guided(tool)) = &self.tools.active else {
            return;
        };
        let guides = tool.guides.clone();
        let Some(edit) = self.session.edit() else {
            return;
        };
        let s = &edit.settings;
        if s.rotate != lf_settings::Rotate::R0 || s.flip_h || s.flip_v {
            toasts.push(Level::Info, GUIDED_TURNED);
            return;
        }
        let aspect = f64::from(full.0) / f64::from(full.1.max(1));
        let Some(fit) = lf_settings::upright::guided(&guides, aspect, &Lens::from_settings(s))
        else {
            return;
        };
        self.commit(toasts);
        self.edit(|s| {
            let mut lens = Lens::from_settings(s);
            lens.vertical = fit.vertical;
            lens.horizontal = fit.horizontal;
            lens.write_into(s);
            let (tw, th) = crop::turned(s, full.0, full.1);
            crop::straighten(s, fit.angle, tw, th);
        });
        self.commit(toasts);
    }

    /// The eyedropper clicked at `at` (source fractions): the temperature and tint that make
    /// it grey, one history step, and the tool closes (as Lightroom's does).
    fn pick_white_balance(&mut self, at: (f64, f64), toasts: &mut Toasts) {
        match self.opened().and_then(|o| auto::pick(&o.sample, at)) {
            Some(found) => {
                self.tools.active = None;
                self.commit(toasts);
                self.edit(|s| (s.temperature, s.tint) = found);
                self.commit(toasts);
            }
            None => toasts.push(Level::Info, auto::TOO_DARK),
        }
    }

    /// Alt-click with a colour range: the selected mask's range takes the hue at `at` (source
    /// fractions), one history step.
    fn pick_range_colour(&mut self, at: (f64, f64), toasts: &mut Toasts) {
        let Some(Active::Masks(tool)) = &self.tools.active else {
            return;
        };
        let Some(index) = tool.selected else {
            return;
        };
        // The sample is linear light; the range is measured on the photo's own (gamma) values.
        let hue = self
            .opened()
            .and_then(|o| o.sample.linear_at(at, 2))
            .and_then(|lin| lf_settings::masks::hue_of(lin.map(|v| v.max(0.0).powf(1.0 / 2.2))));
        let Some(picked) = hue else {
            toasts.push(Level::Info, masks::NO_HUE);
            return;
        };
        self.commit(toasts);
        self.edit(|s| {
            let mut list = Masks::from_settings(s);
            if let Some(MaskRange::Colour { hue, .. }) = list.0.get_mut(index).map(|m| &mut m.range)
            {
                *hue = picked.round();
                list.write_into(s);
            }
        });
        self.commit(toasts);
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
            Active::WhiteBalance => {}
            Active::Guided(tool) => {
                let m =
                    SpotMap::new(map, &edit.settings, full, output_w).through_lens(&edit.settings);
                let lines = tool.guides.iter().map(|g| (g.a, g.b)).chain(tool.drawing);
                for (a, b) in lines {
                    let (p, q) = (m.screen(a), m.screen(b));
                    painter.line_segment([p, q], Stroke::new(3.0, Color32::from_black_alpha(110)));
                    painter.line_segment([p, q], Stroke::new(1.5, Color32::from_rgb(255, 210, 60)));
                    for end in [p, q] {
                        painter.circle_filled(end, 3.5, Color32::from_rgb(255, 210, 60));
                    }
                }
            }
            Active::Masks(tool) => {
                let spot_map = SpotMap::new(map, &edit.settings, full, output_w);
                let masks = Masks::from_settings(&edit.settings);
                for (i, m) in masks.0.iter().enumerate() {
                    paint_mask(&painter, &spot_map, m, tool.selected == Some(i));
                }
                // The brush, where it would paint.
                if tool.brush_selected(&masks).is_some()
                    && let Some(at) = ui.ctx().pointer_hover_pos().filter(|p| area.contains(*p))
                {
                    let r = (tool.brush.size / spot_map.aspect * spot_map.scale) as f32;
                    let inner = r * (1.0 - tool.brush.feather as f32 / 100.0);
                    painter.circle_stroke(
                        at,
                        r + 1.0,
                        Stroke::new(3.0, Color32::from_black_alpha(90)),
                    );
                    painter.circle_stroke(at, r, Stroke::new(1.5, Color32::WHITE));
                    if inner > 2.0 && inner < r - 2.0 {
                        painter.circle_stroke(
                            at,
                            inner,
                            Stroke::new(1.0, Color32::from_white_alpha(140)),
                        );
                    }
                }
            }
            Active::Spots(tool) => {
                let spot_map =
                    SpotMap::new(map, &edit.settings, full, output_w).through_lens(&edit.settings);
                for (i, s) in edit.settings.spots.iter().enumerate() {
                    paint_spot(&painter, &spot_map, s, tool.selected == Some(i));
                }
                let heals = Heals::from_settings(&edit.settings);
                for (i, s) in heals.0.iter().enumerate() {
                    paint_stroke(&painter, &spot_map, s, tool.heal_selected == Some(i));
                }
                if let Some(points) = &tool.stroke {
                    let line: Vec<Pos2> = points
                        .iter()
                        .map(|p| spot_map.screen((p[0], p[1])))
                        .collect();
                    let width = ((tool.brush_r * spot_map.scale) as f32 * 2.0).max(2.0);
                    painter.add(egui::Shape::line(
                        line,
                        Stroke::new(width, Color32::from_rgba_unmultiplied(230, 60, 60, 110)),
                    ));
                }
            }
        }
    }

    /// The tool buttons and the open tool's panel (above the adjust panels).
    pub(super) fn tool_ui(&mut self, ui: &mut egui::Ui, toasts: &mut Toasts) {
        let open = self.tool();
        let local_ai = self.local_ai;
        let mut toggle = None;
        // Wrapped: four tools are close to the side panel's width.
        ui.horizontal_wrapped(|ui| {
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
            let wb = ui
                .selectable_label(open == Some(ToolKind::WhiteBalance), WHITE_BALANCE)
                .on_hover_text("Pick a neutral colour (W)");
            if wb.clicked() {
                toggle = Some(ToolKind::WhiteBalance);
            }
            let masks = ui
                .selectable_label(open == Some(ToolKind::Masks), MASKS)
                .on_hover_text("Local adjustments: linear and radial gradients (M)");
            if masks.clicked() {
                toggle = Some(ToolKind::Masks);
            }
            let guided = ui
                .selectable_label(open == Some(ToolKind::Guided), GUIDED)
                .on_hover_text("Guided Upright: draw lines that should be upright or level");
            if guided.clicked() {
                toggle = Some(ToolKind::Guided);
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
                edit.set(|s| {
                    let strokes = Heals::from_settings(s).0.len();
                    let (o, remove) = spots_panel(ui, tool, &mut s.spots, strokes);
                    out = o;
                    if remove {
                        out = out.or(remove_stroke(tool, s));
                    }
                });
            }
            Active::WhiteBalance => {
                ui.label(RichText::new(WB_HINT).color(theme::MUTED));
            }
            Active::Guided(tool) => {
                ui.label(RichText::new(GUIDED_HINT).color(theme::MUTED));
                let n = tool.guides.len();
                ui.label(match n {
                    0 => "No guides yet.".to_owned(),
                    1 => "1 guide.".to_owned(),
                    n => format!("{n} guides."),
                });
                if ui
                    .add_enabled(n > 0, egui::Button::new(CLEAR_GUIDES))
                    .on_hover_text("The correction stays; undo takes it back")
                    .clicked()
                {
                    tool.guides.clear();
                }
            }
            Active::Masks(tool) => {
                let mut refused = None;
                tool.ai_allowed = local_ai;
                edit.set(|s| {
                    let mut masks = Masks::from_settings(s);
                    let (o, problem) = masks_panel(ui, tool, &mut masks);
                    if o.changed {
                        masks.write_into(s);
                    }
                    out = o;
                    refused = problem;
                });
                if let Some(problem) = refused {
                    toasts.push(Level::Warning, problem);
                }
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

/// A mask's exposure range (stops) and its turn (degrees).
const MASK_EXPOSURE: Range = Range::new(-4.0, 4.0).step(0.05, 2);
const MASK_ANGLE: Range = Range::new(0.0, 360.0).step(1.0, 0);
/// The brush's size (percent of the photo's height, each side of the stroke) and flow.
const BRUSH_SIZE: Range = Range::new(0.5, 30.0).step(0.5, 1);
const BRUSH_FLOW: Range = Range::new(1.0, 100.0).step(1.0, 0);
/// A mask range's percentages and hues.
const RANGE_PERCENT: Range = Range::new(0.0, 100.0).step(1.0, 0);
const RANGE_HUE: Range = Range::new(0.0, 359.0).step(1.0, 0);
const RANGE_WIDTH: Range = Range::new(1.0, 180.0).step(1.0, 0);

/// The masks panel: the hint, the add buttons, the list (click to pick one) and the picked
/// mask's own settings. Returns what changed, and why an add was refused.
fn masks_panel(
    ui: &mut egui::Ui,
    tool: &mut MaskTool,
    list: &mut Masks,
) -> (ToolOutput, Option<&'static str>) {
    let step = ToolOutput {
        changed: true,
        commit: true,
    };
    let mut out = ToolOutput::default();
    let mut problem = None;
    ui.label(RichText::new(masks::HINT).color(theme::MUTED));
    // Wrapped: three buttons are close to the side panel's width.
    ui.horizontal_wrapped(|ui| {
        for label in [masks::ADD_LINEAR, masks::ADD_RADIAL, masks::ADD_BRUSH] {
            if ui.button(label).clicked() {
                let mask = match label {
                    masks::ADD_LINEAR => Mask::linear(),
                    masks::ADD_RADIAL => Mask::radial(),
                    _ => Mask::brush(),
                };
                match tool.add(list, mask) {
                    Ok(()) => out = step,
                    Err(e) => problem = Some(e),
                }
            }
        }
    });
    // AI: the subject, the sky, or people and their parts, found in the photo (on this
    // computer).
    ui.horizontal_wrapped(|ui| {
        for target in lf_ai::segment::Target::ALL {
            if target == lf_ai::segment::Target::People {
                ui.end_row();
                ui.label(egui::RichText::new(masks::PEOPLE).color(crate::theme::MUTED));
            }
            if target == lf_ai::segment::Target::Eyes {
                ui.end_row();
                ui.label(egui::RichText::new(masks::FACE).color(crate::theme::MUTED));
            }
            let button = ui.add_enabled(tool.ai_allowed, egui::Button::new(target.button()));
            if button.on_disabled_hover_text(masks::AI_OFF).clicked() {
                tool.ai_request = Some(target);
            }
            if target == lf_ai::segment::Target::Sky {
                let object = ui
                    .add_enabled(
                        tool.ai_allowed,
                        egui::Button::selectable(tool.picking_object, masks::SELECT_OBJECT),
                    )
                    .on_disabled_hover_text(masks::AI_OFF);
                if object.clicked() {
                    tool.picking_object = !tool.picking_object;
                }
            }
        }
    });
    if tool.picking_object {
        ui.label(egui::RichText::new(masks::PICK_OBJECT).color(crate::theme::WARNING));
    }
    for (i, m) in list.0.iter().enumerate() {
        let name = format!("{}. {}", i + 1, m.kind_name());
        if ui
            .selectable_label(tool.selected == Some(i), name)
            .clicked()
        {
            tool.selected = Some(i);
        }
    }
    let Some(i) = tool.selected.filter(|&i| i < list.0.len()) else {
        return (out, problem);
    };
    ui.separator();
    let m = &mut list.0[i];
    if ui.checkbox(&mut m.invert, masks::INVERT).changed() {
        out = step;
    }
    let mut absorb = |r: panels::slider::SliderResponse| {
        out.changed |= r.changed;
        out.commit |= r.released;
    };
    if let Shape::Brush { .. } = m.shape {
        ui.label(RichText::new(masks::BRUSH_HINT).color(theme::MUTED));
        let tip = &mut tool.brush;
        let mut size = tip.size * 100.0;
        panels::slider::edit(ui, "Size", &mut size, BRUSH_SIZE, None);
        tip.size = size / 100.0;
        panels::slider::edit(ui, "Feather", &mut tip.feather, Range::AMOUNT, None);
        panels::slider::edit(ui, "Flow", &mut tip.flow, BRUSH_FLOW, None);
        ui.checkbox(&mut tip.erase, masks::ERASE);
        ui.separator();
    }
    if let Shape::Radial { feather, angle, .. } = &mut m.shape {
        absorb(panels::slider::edit(
            ui,
            "Feather",
            feather,
            Range::AMOUNT,
            None,
        ));
        absorb(panels::slider::edit(ui, "Angle", angle, MASK_ANGLE, None));
    }
    // Range: only some tones or colours inside the shape.
    let before = m.range;
    let combo = egui::ComboBox::from_id_salt("mask-range")
        .selected_text(m.range.name())
        .show_ui(ui, |ui| {
            for choice in [MaskRange::All, MaskRange::luminance(), MaskRange::colour()] {
                let same = std::mem::discriminant(&m.range) == std::mem::discriminant(&choice);
                if ui.selectable_label(same, choice.name()).clicked() && !same {
                    m.range = choice;
                }
            }
        });
    combo
        .response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, masks::RANGE));
    if m.range != before {
        absorb(panels::slider::SliderResponse {
            changed: true,
            released: true,
        });
    }
    match &mut m.range {
        MaskRange::All => {}
        MaskRange::Luminance { low, high, smooth } => {
            absorb(panels::slider::edit(
                ui,
                "Darkest",
                low,
                RANGE_PERCENT,
                None,
            ));
            absorb(panels::slider::edit(
                ui,
                "Brightest",
                high,
                RANGE_PERCENT,
                None,
            ));
            absorb(panels::slider::edit(
                ui,
                "Smoothness",
                smooth,
                RANGE_PERCENT,
                None,
            ));
            if *high < *low {
                *high = *low;
            }
        }
        MaskRange::Colour { hue, width, smooth } => {
            ui.horizontal(|ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                let rgb = egui::ecolor::Hsva::new((*hue / 360.0) as f32, 0.8, 0.9, 1.0);
                ui.painter().rect_filled(rect, 3.0, Color32::from(rgb));
                ui.label(RichText::new(masks::RANGE_PICK).color(theme::MUTED).small());
            });
            absorb(panels::slider::edit(ui, "Hue", hue, RANGE_HUE, None));
            absorb(panels::slider::edit(ui, "Range", width, RANGE_WIDTH, None));
            absorb(panels::slider::edit(
                ui,
                "Smoothness",
                smooth,
                RANGE_WIDTH,
                None,
            ));
        }
    }
    ui.separator();
    for (label, value) in m.local.sliders() {
        let range = if label == "Exposure" {
            MASK_EXPOSURE
        } else {
            Range::BIPOLAR
        };
        absorb(panels::slider::edit(ui, label, value, range, None));
    }
    if ui.button(masks::REMOVE).on_hover_text("Delete").clicked() && tool.remove_selected(list) {
        out = step;
    }
    (out, problem)
}

/// A mask over the photo: a radial's outline, a linear's three lines, and the handles. The
/// mask being edited is drawn in the brand colour.
fn paint_mask(painter: &egui::Painter, map: &SpotMap, m: &Mask, selected: bool) {
    let colour = if selected {
        theme::BRAND_CONTRAST
    } else {
        Color32::from_white_alpha(150)
    };
    let stroke = Stroke::new(if selected { 2.0 } else { 1.0 }, colour);
    let shadow = Stroke::new(stroke.width + 1.5, Color32::from_black_alpha(90));
    let handle = |at: (f64, f64)| {
        let p = map.screen(at);
        let r = if selected { 5.0 } else { 3.5 };
        painter.circle(
            p,
            r,
            if selected {
                theme::BRAND
            } else {
                Color32::WHITE
            },
            Stroke::new(1.5, colour),
        );
    };
    match &m.shape {
        &Shape::Linear { x0, y0, x1, y1 } => {
            // Lines long enough to cross any photo (they are clipped to it).
            if let Some(lines) = masks::linear_lines(&m.shape, map.aspect, 3.0) {
                for (k, [a, b]) in lines.into_iter().enumerate() {
                    let (a, b) = (map.screen(a), map.screen(b));
                    painter.line_segment([a, b], shadow);
                    if k == 1 {
                        painter.extend(egui::Shape::dashed_line(&[a, b], stroke, 6.0, 4.0));
                    } else {
                        painter.line_segment([a, b], stroke);
                    }
                }
            }
            handle((x0, y0));
            handle((x1, y1));
            handle(((x0 + x1) / 2.0, (y0 + y1) / 2.0));
        }
        Shape::Brush { strokes } => {
            // Only the mask being painted shows where it reaches.
            if selected {
                paint_coverage(painter, map, strokes);
            }
        }
        // An AI mask has no handles: it follows the photo.
        Shape::Image { .. } => {}
        Shape::Radial { .. } => {
            let ring: Vec<Pos2> = masks::radial_outline(&m.shape, map.aspect, 96)
                .into_iter()
                .map(|p| map.screen(p))
                .collect();
            painter.add(egui::Shape::closed_line(ring.clone(), shadow));
            painter.add(egui::Shape::closed_line(ring, stroke));
            if let Some(handles) = masks::radial_handles(&m.shape, map.aspect) {
                for at in handles {
                    handle(at);
                }
            }
        }
    }
}

/// The long edge of the painted-area overlay, in pixels (it is soft; the photo shows the rest).
const COVERAGE_EDGE: usize = 256;

/// Where a brush mask reaches, as a red tint over the photo: its strokes drawn by the same
/// rasterizer the develop pass uses, into a small texture kept (in egui's memory) until the
/// strokes change, laid on the photo's four corners.
fn paint_coverage(painter: &egui::Painter, map: &SpotMap, strokes: &[lf_settings::masks::Stroke]) {
    use std::hash::{Hash, Hasher};
    if strokes.is_empty() {
        return;
    }
    let (w, h) = if map.aspect >= 1.0 {
        (
            COVERAGE_EDGE,
            (COVERAGE_EDGE as f64 / map.aspect).round().max(1.0) as usize,
        )
    } else {
        (
            (COVERAGE_EDGE as f64 * map.aspect).round().max(1.0) as usize,
            COVERAGE_EDGE,
        )
    };
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(strokes)
        .unwrap_or_default()
        .hash(&mut hasher);
    (w, h).hash(&mut hasher);
    let key = hasher.finish();
    let id = egui::Id::unique("brush-coverage");
    let ctx = painter.ctx();
    let cached: Option<(u64, egui::TextureHandle)> = ctx.data(|d| d.get_temp(id));
    let texture = match cached {
        Some((k, t)) if k == key => t,
        _ => {
            let cover = lf_settings::masks::rasterize(strokes, w, h);
            let pixels = cover
                .iter()
                .map(|&a| {
                    Color32::from_rgba_unmultiplied(230, 40, 40, (u32::from(a) * 110 / 255) as u8)
                })
                .collect();
            let image = egui::ColorImage::new([w, h], pixels);
            let t = ctx.load_texture("brush-coverage", image, egui::TextureOptions::LINEAR);
            ctx.data_mut(|d| d.insert_temp(id, (key, t.clone())));
            t
        }
    };
    let mut mesh = egui::Mesh::with_texture(texture.id());
    for (corner, uv) in [
        ((0.0, 0.0), egui::pos2(0.0, 0.0)),
        ((1.0, 0.0), egui::pos2(1.0, 0.0)),
        ((1.0, 1.0), egui::pos2(1.0, 1.0)),
        ((0.0, 1.0), egui::pos2(0.0, 1.0)),
    ] {
        mesh.vertices.push(egui::epaint::Vertex {
            pos: map.screen(corner),
            uv,
            color: Color32::WHITE,
        });
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
}

/// The spot tool's panel: the hint, Heal / Clone, the count and "Remove spot".
/// Removes the picked healing stroke.
fn remove_stroke(tool: &mut SpotTool, s: &mut lf_settings::EditSettings) -> ToolOutput {
    let mut heals = Heals::from_settings(s);
    let Some(i) = tool.heal_selected.filter(|&i| i < heals.0.len()) else {
        return ToolOutput::default();
    };
    heals.0.remove(i);
    heals.write_into(s);
    tool.heal_selected = None;
    ToolOutput {
        changed: true,
        commit: true,
    }
}

/// The healing brush's size (percent of the photo's width).
const HEAL_SIZE: Range = Range::new(0.2, 10.0).step(0.1, 1);

fn spots_panel(
    ui: &mut egui::Ui,
    tool: &mut SpotTool,
    spots: &mut Vec<Spot>,
    settings_strokes: usize,
) -> (ToolOutput, bool) {
    let mut remove_stroke = false;
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
    ui.checkbox(&mut tool.brush, spots::BRUSH)
        .on_hover_text("Paint along a hair, a wire or a scratch; it is healed from beside it");
    if tool.brush {
        let mut size = tool.brush_r * 100.0;
        panels::slider::edit(ui, "Size", &mut size, HEAL_SIZE, None);
        tool.brush_r =
            (size / 100.0).clamp(lf_settings::heal::MIN_RADIUS, lf_settings::heal::MAX_RADIUS);
        ui.label(RichText::new(spots::BRUSH_HINT).color(theme::MUTED));
    }
    ui.label(RichText::new(format!("{} of {MAX_SPOTS} spots", spots.len())).color(theme::MUTED));
    if settings_strokes > 0 {
        ui.label(RichText::new(format!("{settings_strokes} healing strokes")).color(theme::MUTED));
    }
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
    if tool.brush
        && ui
            .add_enabled(
                tool.heal_selected.is_some(),
                egui::Button::new(spots::REMOVE_STROKE),
            )
            .on_hover_text("Delete")
            .clicked()
    {
        remove_stroke = true;
    }
    (out, remove_stroke)
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

/// A healing stroke: the line it heals (brand when picked) and the line it copies from,
/// dashed, joined at the start.
fn paint_stroke(painter: &egui::Painter, map: &SpotMap, s: &HealStroke, selected: bool) {
    let line: Vec<Pos2> = s.points.iter().map(|p| map.screen((p[0], p[1]))).collect();
    let source: Vec<Pos2> = s
        .points
        .iter()
        .map(|p| map.screen((p[0] + s.dx, p[1] + s.dy)))
        .collect();
    let width = ((s.r * map.scale) as f32 * 2.0).max(2.0);
    let colour = if selected {
        theme::BRAND
    } else {
        Color32::WHITE
    };
    if let (Some(a), Some(b)) = (line.first(), source.first()) {
        painter.line_segment([*a, *b], Stroke::new(1.0, Color32::from_white_alpha(150)));
    }
    painter.add(egui::Shape::line(
        line,
        Stroke::new(width, colour.gamma_multiply(0.35)),
    ));
    painter.extend(egui::Shape::dashed_line(
        &source,
        Stroke::new(
            1.5,
            Color32::from_white_alpha(if selected { 255 } else { 170 }),
        ),
        4.0,
        3.0,
    ));
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

    #[test]
    fn through_the_lens_a_spot_lands_where_the_shader_samples() {
        let mut s = EditSettings::default_settings();
        let lens = Lens {
            distortion: 70.0,
            vertical: 40.0,
            ..Lens::default()
        };
        lens.write_into(&mut s);
        let screen = ScreenMap {
            shown: Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1500.0, 1000.0)),
            view: View::default(),
        };
        let plain = SpotMap::new(screen, &s, (3000, 2000), 3000.0);
        let spots = SpotMap::new(screen, &s, (3000, 2000), 3000.0).through_lens(&s);
        let at = egui::pos2(1300.0, 150.0);
        // The corrected point the masks use, and the photo-as-shot point the spots use.
        let corrected = plain.uv(at);
        let shot = spots.uv(at);
        assert_eq!(shot, lens.to_source(corrected, 1.5));
        assert!(shot != corrected, "the correction moves it");
        // Back to the same screen point both ways.
        let back = spots.screen(shot);
        assert!(
            (back.x - at.x).abs() < 1e-3 && (back.y - at.y).abs() < 1e-3,
            "{back:?}"
        );
        // No correction that moves pixels: the plain map.
        let mut vignette_only = EditSettings::default_settings();
        Lens {
            vignetting: 50.0,
            ..Lens::default()
        }
        .write_into(&mut vignette_only);
        let m =
            SpotMap::new(screen, &vignette_only, (3000, 2000), 3000.0).through_lens(&vignette_only);
        assert!(m.lens.is_none());
    }
}
