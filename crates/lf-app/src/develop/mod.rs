//! The Develop view: the open photo drawn by its `Renderer` straight into a texture egui
//! shows (no readback while editing), with zoom and pan, before/after, the histogram and the
//! info strip.
//!
//! Opening runs on the `Open` lane ([`open::run`]): the sidecar is reconciled, the photo is
//! decoded (through the frame cache for RAW) and its renderer built there, so the UI thread
//! receives a renderer ready to draw. Each request bumps a generation; an answer from an
//! older one is dropped.

pub mod histogram;
pub mod info;
pub mod open;
pub mod preview;
pub mod view;

use crate::app::DEVICE_LOST_BANNER;
use crate::jobs::{Jobs, Lane};
use crate::theme;
use crate::toast::{Level, Toasts};
use eframe::egui::{self, Key, Modifiers, RichText, Sense};
use eframe::egui_wgpu::RenderState;
use histogram::Throttle;
use lf_catalog::PhotoRow;
use lf_pipeline::renderer::{DrawOptions, Histogram, View};
use lf_pipeline::{Gpu, PipelineError};
use lf_raw::cache::FrameCache;
use lf_raw::demosaic::Quality;
use open::OpenedPhoto;
use preview::Preview;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use view::Viewport;

/// Shown while a photo opens.
pub const OPENING: &str = "Opening…";
/// The placeholder for a photo that cannot be opened.
pub const CANT_OPEN: &str = "Can't open";
/// The corner label while the photo is shown as it was.
pub const BEFORE: &str = "Before";
/// Shown where the preview would be when the window has no graphics device to share.
pub const NO_PREVIEW: &str = "The preview needs the graphics device.";

/// The start of the toast for a photo that cannot be opened.
pub fn cant_open_text(message: &str) -> String {
    format!("This file can't be opened: {message}")
}

/// What the view is showing.
enum State {
    Empty,
    Opening(i64),
    Ready(Box<OpenedPhoto>, i64),
    Failed(i64, String),
    /// The graphics device was lost: the renderer is gone until the app restarts.
    Lost,
}

/// What a drawn frame depended on: drawn again when any of it changes.
#[derive(Debug, Clone, Copy, PartialEq)]
struct DrawKey {
    size: (u32, u32),
    view: View,
    before: bool,
}

/// What became of an open answer, for the library (badges, neighbours).
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Shown: pre-open its neighbours.
    Shown(i64),
    /// Could not be opened: badge the cell.
    Failed(i64, String),
    /// Stale, or the device is gone.
    Dropped,
}

pub struct Develop {
    gpu: Arc<Gpu>,
    render_state: Option<RenderState>,
    /// The latest request's generation, shared with the open jobs.
    latest: Arc<AtomicU64>,
    state: State,
    viewport: Viewport,
    /// "Before" clicked on (the `\` key shows it only while held).
    before_toggled: bool,
    before_held: bool,
    preview: Option<Preview>,
    last_drawn: Option<DrawKey>,
    /// The settings changed: draw again.
    dirty: bool,
    histogram: Option<Histogram>,
    last_change: Instant,
    last_histogram: Option<Instant>,
    lost_logged: bool,
}

impl Develop {
    pub fn new(gpu: Arc<Gpu>) -> Develop {
        Develop {
            gpu,
            render_state: None,
            latest: Arc::new(AtomicU64::new(0)),
            state: State::Empty,
            viewport: Viewport::default(),
            before_toggled: false,
            before_held: false,
            preview: None,
            last_drawn: None,
            dirty: false,
            histogram: None,
            last_change: Instant::now(),
            last_histogram: None,
            lost_logged: false,
        }
    }

    /// egui's renderer, to register the preview canvas with (none in headless tests).
    pub fn set_render_state(&mut self, render_state: Option<RenderState>) {
        self.render_state = render_state;
    }

    /// The photo shown or being opened.
    pub fn photo_id(&self) -> Option<i64> {
        match &self.state {
            State::Opening(id) | State::Ready(_, id) | State::Failed(id, _) => Some(*id),
            State::Empty | State::Lost => None,
        }
    }

    /// A renderer is held (the photo is ready to draw).
    pub fn is_ready(&self) -> bool {
        matches!(self.state, State::Ready(..))
    }

    pub fn is_lost(&self) -> bool {
        matches!(self.state, State::Lost)
    }

    /// The open photo, once ready.
    pub fn opened(&self) -> Option<&OpenedPhoto> {
        match &self.state {
            State::Ready(photo, _) => Some(photo),
            _ => None,
        }
    }

    /// The generation the next answer must carry to be shown.
    pub fn generation(&self) -> u64 {
        self.latest.load(Ordering::SeqCst)
    }

    /// Before/after is showing the photo as it was.
    pub fn before(&self) -> bool {
        self.before_toggled || self.before_held
    }

    /// Opens `photo` (no-op when it is already shown or opening). On a lost device nothing
    /// is opened: the view says why.
    pub fn request(
        &mut self,
        photo: &PhotoRow,
        root: &Path,
        cache: Option<Arc<FrameCache>>,
        quality: Quality,
        jobs: &Jobs,
    ) {
        if self.photo_id() == Some(photo.id) && !matches!(self.state, State::Failed(..)) {
            return;
        }
        if self.check_lost() {
            return;
        }
        let generation = self.latest.fetch_add(1, Ordering::SeqCst) + 1;
        self.state = State::Opening(photo.id);
        self.preview = None;
        self.histogram = None;
        self.viewport = Viewport::default();
        self.before_toggled = false;
        let req = open::Request {
            photo_id: photo.id,
            path: photo.path.clone(),
            root: root.to_path_buf(),
            gpu: self.gpu.clone(),
            cache,
            quality,
            max_edge: open::max_edge(&self.gpu),
            generation,
            latest: self.latest.clone(),
        };
        jobs.spawn(Lane::Open, "open photo", move || open::run(req));
    }

    /// Decodes `photos` (RAW, present) into the frame cache, after the current one.
    pub fn preopen(
        &self,
        photos: &[&PhotoRow],
        root: &Path,
        cache: Option<Arc<FrameCache>>,
        quality: Quality,
        jobs: &Jobs,
    ) {
        let Some(cache) = cache else {
            return;
        };
        for photo in photos.iter().filter(|p| p.kind == "raw" && !p.missing) {
            let req = open::Request {
                photo_id: photo.id,
                path: photo.path.clone(),
                root: root.to_path_buf(),
                gpu: self.gpu.clone(),
                cache: Some(cache.clone()),
                quality,
                max_edge: open::max_edge(&self.gpu),
                generation: self.generation(),
                latest: self.latest.clone(),
            };
            jobs.spawn(Lane::Open, "pre-open photo", move || open::preopen(req));
        }
    }

    /// Leaves Develop: the renderer and canvas are freed; queued opens are skipped.
    pub fn close(&mut self) {
        self.latest.fetch_add(1, Ordering::SeqCst);
        self.preview = None;
        self.last_drawn = None;
        self.histogram = None;
        if !self.is_lost() {
            self.state = State::Empty;
        }
    }

    /// An open answer arrived.
    pub fn handle_opened(
        &mut self,
        photo_id: i64,
        generation: u64,
        result: Result<Box<OpenedPhoto>, String>,
        toasts: &mut Toasts,
    ) -> Outcome {
        if generation != self.generation() {
            log::debug!("dropping the open of photo {photo_id} from an earlier request");
            return Outcome::Dropped;
        }
        if self.check_lost() {
            return Outcome::Dropped;
        }
        match result {
            Ok(photo) => {
                self.state = State::Ready(photo, photo_id);
                self.viewport = Viewport::default();
                self.last_drawn = None;
                self.dirty = true;
                self.histogram = None;
                self.last_change = Instant::now();
                self.last_histogram = None;
                Outcome::Shown(photo_id)
            }
            Err(message) => {
                log::warn!("photo {photo_id} could not be opened: {message}");
                toasts.push(Level::Error, cant_open_text(&message));
                self.state = State::Failed(photo_id, message.clone());
                Outcome::Failed(photo_id, message)
            }
        }
    }

    /// The open job panicked: the photo being opened shows as one that cannot be opened (the
    /// app's own "Something went wrong" toast tells the user).
    pub fn open_failed(&mut self, message: &str) {
        if let State::Opening(id) = self.state {
            self.state = State::Failed(id, message.to_owned());
        }
    }

    /// When the device is lost: drop the renderer and canvas (once), log it, and show the
    /// lost state. True when lost.
    fn check_lost(&mut self) -> bool {
        if !self.gpu.is_lost() {
            return false;
        }
        if !self.lost_logged {
            log::error!("the graphics device was lost: the Develop view dropped its renderer");
            self.lost_logged = true;
        }
        self.preview = None;
        self.last_drawn = None;
        self.state = State::Lost;
        true
    }

    /// Handles a renderer error: a lost device takes the device-lost path; anything else is
    /// logged and shown once (not redrawn until something changes).
    fn renderer_error(&mut self, what: &str, e: PipelineError, toasts: &mut Toasts) {
        if matches!(e, PipelineError::DeviceLost) || self.gpu.is_lost() {
            self.check_lost();
            return;
        }
        log::error!("{what}: {e}");
        toasts.push(Level::Error, format!("{what}. {e}"));
    }

    /// The side panel: histogram, info strip and the Before button.
    pub fn side_ui(&mut self, ui: &mut egui::Ui, photo: Option<&PhotoRow>) {
        ui.add_space(8.0);
        histogram::show(ui, self.histogram.as_ref());
        ui.add_space(8.0);
        if let State::Ready(opened, _) = &self.state {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(info::kind_badge(opened.kind))
                        .strong()
                        .background_color(theme::SURFACE_2),
                );
                ui.label(RichText::new(&opened.file_name).strong());
            });
            ui.label(info::dimensions(opened.full_width, opened.full_height));
            for line in info::lines(&opened.exif) {
                ui.label(RichText::new(line).color(theme::MUTED));
            }
            if opened.reduced {
                let edge = opened.renderer.width.max(opened.renderer.height);
                ui.label(RichText::new(info::reduced_note(edge)).color(theme::WARNING));
            }
            if let Some(error) = &opened.sidecar_error {
                ui.add_space(8.0);
                ui.label(RichText::new(error).color(theme::WARNING));
            }
            ui.add_space(12.0);
            let before = self.before();
            if ui.selectable_label(before, BEFORE).clicked() {
                self.before_toggled = !self.before_toggled;
            }
        } else if let Some(photo) = photo {
            ui.label(RichText::new(open::file_name(&photo.path)).strong());
        }
    }

    /// The preview area: draws the photo, handles zoom, pan and before/after.
    pub fn preview_ui(&mut self, ui: &mut egui::Ui, toasts: &mut Toasts) {
        self.check_lost();
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        match &self.state {
            State::Empty => return,
            State::Opening(_) => {
                centred(ui, rect, |ui| {
                    ui.spinner();
                    ui.label(RichText::new(OPENING).color(theme::MUTED));
                });
                return;
            }
            State::Failed(_, message) => {
                let message = message.clone();
                centred(ui, rect, |ui| {
                    ui.heading(CANT_OPEN);
                    ui.label(RichText::new(message).color(theme::MUTED));
                });
                return;
            }
            State::Lost => {
                centred(ui, rect, |ui| {
                    ui.label(RichText::new(DEVICE_LOST_BANNER).color(theme::DANGER));
                });
                return;
            }
            State::Ready(..) => {}
        }
        let ppp = ui.ctx().pixels_per_point();
        let panel = (
            f64::from(rect.width() * ppp),
            f64::from(rect.height() * ppp),
        );
        if panel.0 < 1.0 || panel.1 < 1.0 {
            return;
        }
        let output = match &self.state {
            State::Ready(photo, _) => {
                let (w, h) = photo.renderer.output_size(&photo.settings);
                (f64::from(w), f64::from(h))
            }
            _ => return,
        };
        self.interact(ui, rect, &response, output, panel);

        let Some(render_state) = self.render_state.clone() else {
            centred(ui, rect, |ui| {
                ui.label(RichText::new(NO_PREVIEW).color(theme::MUTED));
            });
            return;
        };
        let placement = self.viewport.placement(output, panel);
        let (rx, ry, rw, rh) = placement.rect;
        let size = ((rw.round() as u32).max(1), (rh.round() as u32).max(1));
        let before = self.before();
        let key = DrawKey {
            size,
            view: placement.view,
            before,
        };
        let preview = self
            .preview
            .get_or_insert_with(|| Preview::new(render_state));
        let (canvas, texture, fresh) = match preview.ensure(&self.gpu, size.0, size.1) {
            Ok(made) => made,
            Err(e) => {
                self.preview = None;
                self.renderer_error("The preview could not be made", e, toasts);
                return;
            }
        };
        let mut failed = None;
        if fresh || self.dirty || self.last_drawn != Some(key) {
            if let State::Ready(photo, _) = &mut self.state {
                let opts = DrawOptions {
                    before,
                    crop: None,
                    view: placement.view,
                };
                if let Err(e) = photo.renderer.draw(&photo.settings, &opts, canvas) {
                    failed = Some(e);
                }
            }
            self.dirty = false;
            self.last_drawn = Some(key);
        }
        if let Some(e) = failed {
            self.renderer_error("The preview could not be drawn", e, toasts);
            if self.is_lost() {
                return;
            }
        }
        let shown = egui::Rect::from_min_size(
            rect.min + egui::vec2((rx / f64::from(ppp)) as f32, (ry / f64::from(ppp)) as f32),
            egui::vec2((rw / f64::from(ppp)) as f32, (rh / f64::from(ppp)) as f32),
        );
        ui.painter().image(
            texture,
            shown,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
        if before {
            let at = rect.left_top() + egui::vec2(12.0, 12.0);
            let galley = ui.painter().layout_no_wrap(
                BEFORE.to_owned(),
                egui::FontId::proportional(14.0),
                theme::INK,
            );
            let back = egui::Rect::from_min_size(at, galley.size()).expand(6.0);
            ui.painter().rect_filled(back, 4.0, theme::SURFACE);
            ui.painter().galley(at, galley, theme::INK);
        }
        self.update_histogram(ui.ctx(), toasts);
    }

    /// Scroll zooms at the cursor, drag pans, double-click and `Space` toggle fit and 100 %,
    /// `\` held shows the photo as it was.
    fn interact(
        &mut self,
        ui: &egui::Ui,
        rect: egui::Rect,
        response: &egui::Response,
        output: (f64, f64),
        panel: (f64, f64),
    ) {
        let ppp = ui.ctx().pixels_per_point();
        let keys_free = !ui.ctx().egui_wants_keyboard_input();
        let (space, held) = ui.input_mut(|i| {
            (
                keys_free && i.consume_key(Modifiers::NONE, Key::Space),
                keys_free && i.key_down(Key::Backslash),
            )
        });
        self.before_held = held;
        if space || response.double_clicked() {
            self.viewport.toggle_fit_100();
        }
        if let Some(pos) = response.hover_pos() {
            let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta().y, i.zoom_delta()));
            let factor = f64::from(pinch) * (f64::from(scroll) * 0.0015).exp();
            if (factor - 1.0).abs() > 1e-6 {
                let cursor = (
                    f64::from((pos.x - rect.left()) / rect.width()),
                    f64::from((pos.y - rect.top()) / rect.height()),
                );
                self.viewport.zoom_at(factor, cursor, output, panel);
            }
        }
        if response.dragged() {
            let d = response.drag_delta() * ppp;
            self.viewport
                .pan(f64::from(d.x), f64::from(d.y), output, panel);
        }
    }

    /// Remakes the histogram 200 ms after the last change, once per change.
    fn update_histogram(&mut self, ctx: &egui::Context, toasts: &mut Toasts) {
        let now = Instant::now();
        if !Throttle::should_run(self.last_change, self.last_histogram, now) {
            if self.last_histogram.is_none_or(|run| run < self.last_change) {
                let due = (self.last_change + histogram::DELAY).saturating_duration_since(now);
                ctx.request_repaint_after(due);
            }
            return;
        }
        self.last_histogram = Some(now);
        let State::Ready(photo, _) = &mut self.state else {
            return;
        };
        match photo.renderer.histogram(&photo.settings) {
            Ok(h) => {
                self.histogram = Some(h);
                // The side panel was drawn before the preview this frame.
                ctx.request_repaint();
            }
            Err(e) => self.renderer_error("The histogram could not be made", e, toasts),
        }
    }
}

fn centred(ui: &mut egui::Ui, rect: egui::Rect, add: impl FnOnce(&mut egui::Ui)) {
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Center)),
    );
    child.add_space(rect.height() / 3.0);
    add(&mut child);
}
