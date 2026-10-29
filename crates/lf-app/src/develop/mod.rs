//! The Develop view: the open photo drawn by its `Renderer` straight into a texture egui
//! shows (no readback while editing), with zoom and pan, before/after, the histogram and the
//! info strip.
//!
//! Opening runs on the `Open` lane ([`open::run`]): the sidecar is reconciled, the photo is
//! decoded (through the frame cache for RAW) and its renderer built there, so the UI thread
//! receives a renderer ready to draw. Each request bumps a generation; an answer from an
//! older one is dropped.
//!
//! Editing: the adjust panels change the open photo's [`EditState`] on every frame of a
//! gesture (the preview redraws through [`Develop::settings_changed`], the histogram follows
//! 200 ms after the last change), and the gesture's end commits it once to the catalogue
//! (one history step), then hands the sidecar to the debounced [`SidecarSaver`], which
//! writes it on the `Io` lane a quiet second later. Nothing on the drag path renders or
//! reads pixels back (Review Focus 1).
//!
//! The edit state, the catalogue connection and the sidecar writer live in
//! [`editing::EditSession`].

pub mod auto;
pub mod debounce;
pub mod edit_state;
pub mod editing;
pub mod extras;
pub mod histogram;
pub mod history_panel;
pub mod info;
mod navigator;
pub mod open;
pub mod panels;
pub mod preview;
pub mod readout;
pub mod tools;
pub mod upright;
pub mod versions_panel;
pub mod view;

use crate::app::DEVICE_LOST_BANNER;
use crate::jobs::{Jobs, Lane};
use crate::shortcuts::Action;
use crate::theme;
use crate::toast::{Level, Toasts};
use debounce::SidecarSaver;
use edit_state::EditState;
use editing::{EditOp, EditSession};
use eframe::egui::{self, RichText, Sense};
use eframe::egui_wgpu::RenderState;
use histogram::Throttle;
use lf_catalog::PhotoRow;
use lf_pipeline::renderer::{DrawOptions, Histogram, View};
use lf_pipeline::{Gpu, PipelineError};
use lf_raw::cache::FrameCache;
use lf_raw::demosaic::Quality;
use lf_raw::open::Kind;
use lf_settings::DevelopSettings;
use lf_settings::geometry::{self, Geometry};
use lf_settings::groups::GroupKey;
use open::OpenedPhoto;
use panels::{PanelContext, PanelOutput, PanelState};
use preview::Preview;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tools::ToolKind;
use view::{Viewport, Zoom};

/// The space between the preview and the window's edges and the side panel, in points: the
/// photo never sits flush against the adjust panel.
pub const PREVIEW_GUTTER: f32 = 12.0;
/// Shown while a photo opens.
pub const OPENING: &str = "Opening…";
/// The placeholder for a photo that cannot be opened.
pub const CANT_OPEN: &str = "Can't open";
/// The corner label while the photo is shown as it was.
/// Auto-sync's hover text.
pub const AUTO_SYNC_HINT: &str = "Each change to this photo is made on the other selected photos too (not their crop, spots or masks).";
pub const BEFORE: &str = "Before";
/// Shown where the preview would be when the window has no graphics device to share.
pub const AFTER: &str = "After";
pub const NO_PREVIEW: &str = "The preview needs the graphics device.";

pub use editing::{damaged_text, write_failed_text};

/// The damaged-sidecar warning's button.
pub const SHOW_FILE: &str = "Show file";

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
    clipping: bool,
    peaking: bool,
    spots_view: bool,
    proof: bool,
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
    /// View → Clipping (`J`): clipped highlights red, blocked shadows blue, in the preview.
    clipping: bool,
    /// View → Focus peaking (`Shift+J`): sharp edges green, in the preview.
    peaking: bool,
    /// View → Visualize spots: the photo as white edges on black, so dust shows.
    spots_view: bool,
    /// The AI tools may be used (the library passes the gate's answer).
    local_ai: bool,
    /// View → Info: the file's facts over the preview.
    info_overlay: bool,
    /// The History section's rows.
    history: history_panel::HistoryUi,
    /// A preset's adaptive AI masks still to find in the open photo.
    adaptive: extras::AdaptiveQueue,
    /// Soft-proofing through this ICC profile (View → Soft proof), and what the proof LUT was
    /// last built for (profile, look, amount).
    proof: Option<PathBuf>,
    proof_key: Option<(PathBuf, Option<String>, i64)>,
    before_held: bool,
    /// View → Fit, 100 % or `Space`, applied when the preview is next laid out.
    zoom_asked: Option<Action>,
    preview: Option<Preview>,
    last_drawn: Option<DrawKey>,
    /// View → Before/After side by side (`Y`): the photo as it was on the left.
    side_by_side: bool,
    /// The left half's canvas, and what it was last drawn for (its size and view).
    preview_before: Option<Preview>,
    last_before: Option<((u32, u32), View)>,
    /// The Navigator's canvas, what it was drawn for, and the part the preview showed last.
    navigator: Option<Preview>,
    navigator_drawn: Option<navigator::NavKey>,
    last_view: View,
    /// The colour readout: a small picture of the edit, and the photo point under the
    /// pointer while it is over the preview.
    readout_img: Option<image::RgbaImage>,
    readout_at: Option<(f64, f64)>,
    /// The settings changed: draw again.
    dirty: bool,
    histogram: Option<Histogram>,
    last_change: Instant,
    last_histogram: Option<Instant>,
    lost_logged: bool,
    /// The edit state, catalogue connection, sidecar writer and damaged-sidecar warning.
    session: EditSession,
    /// The photo shown or being opened (its sidecar sits beside it).
    photo_path: Option<PathBuf>,
    panels: PanelState,
    /// The open photo must be decoded again (its RAW develop settings changed).
    reopen: bool,
    /// The crop and spot tools.
    tools: tools::Tools,
    /// The Looks, Studio presets and Versions sections.
    extras: extras::Extras,
    /// "Import .cube…" was clicked; the library opens the file picker.
    lut_import_asked: bool,
    /// Auto-sync is on; how many other photos are selected; and the change waiting to go to
    /// them (the photo it was made on, and the keys, [`crate::sync`]).
    auto_sync: bool,
    sync_others: usize,
    sync_out: Option<(i64, serde_json::Value)>,
    /// "Import .dcp…" was clicked (the library takes the request).
    dcp_import_asked: bool,
    /// "Apply lens profile" was clicked (the library takes the request).
    lens_profile_asked: bool,
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
            clipping: false,
            peaking: false,
            spots_view: false,
            local_ai: false,
            adaptive: extras::AdaptiveQueue::default(),
            info_overlay: false,
            history: history_panel::HistoryUi::default(),
            proof: None,
            proof_key: None,
            before_held: false,
            zoom_asked: None,
            preview: None,
            last_drawn: None,
            side_by_side: false,
            preview_before: None,
            last_before: None,
            navigator: None,
            navigator_drawn: None,
            last_view: View::default(),
            readout_img: None,
            readout_at: None,
            dirty: false,
            histogram: None,
            last_change: Instant::now(),
            last_histogram: None,
            lost_logged: false,
            session: EditSession::new(),
            photo_path: None,
            panels: PanelState::default(),
            reopen: false,
            tools: tools::Tools::default(),
            extras: extras::Extras::default(),
            lut_import_asked: false,
            auto_sync: false,
            sync_others: 0,
            sync_out: None,
            dcp_import_asked: false,
            lens_profile_asked: false,
        }
    }

    /// The sidecar writer (the app flushes it when quitting).
    pub fn saver(&self) -> SidecarSaver {
        self.session.saver()
    }

    /// The tone curve point `Delete` would remove.
    pub fn curve_selected(&self) -> Option<usize> {
        self.panels.curve.selected()
    }

    /// The open photo's edit state.
    pub fn edit_state(&self) -> Option<&EditState> {
        self.session.edit()
    }

    /// The one entry point for "the settings changed": the preview draws again and the
    /// histogram is remade 200 ms after the last change.
    pub fn settings_changed(&mut self) {
        self.dirty = true;
        self.last_change = Instant::now();
    }

    /// Changes the live settings (a gesture in progress) and redraws.
    pub fn edit(&mut self, f: impl FnOnce(&mut lf_settings::EditSettings)) {
        if let Some(edit) = self.session.edit_mut() {
            edit.set(f);
            self.settings_changed();
        }
    }

    /// Ends a gesture: one history step when the settings changed.
    pub fn commit(&mut self, toasts: &mut Toasts) {
        self.run_edit(EditOp::Commit, toasts);
    }

    /// Commits a gesture still in progress (leaving the photo, quitting).
    pub fn finish_edit(&mut self, toasts: &mut Toasts) {
        let (name, damaged) = match &self.state {
            State::Ready(o, _) => (o.file_name.clone(), o.sidecar_error.is_some()),
            _ => (
                self.photo_path
                    .as_deref()
                    .map(open::file_name)
                    .unwrap_or_default(),
                false,
            ),
        };
        let committed = self
            .session
            .finish(self.photo_path.as_deref(), damaged, &name, toasts);
        if committed {
            self.check_develop_change();
        }
    }

    pub fn undo(&mut self, toasts: &mut Toasts) {
        self.run_edit(EditOp::Undo, toasts);
    }

    pub fn redo(&mut self, toasts: &mut Toasts) {
        self.run_edit(EditOp::Redo, toasts);
    }

    /// The History panel: back (or forward) to step `id`.
    pub fn jump_history(&mut self, id: i64, toasts: &mut Toasts) {
        self.run_edit(EditOp::Jump(id), toasts);
    }

    /// Puts `groups` back to their defaults (one history step).
    pub fn reset_groups(&mut self, groups: &'static [GroupKey], toasts: &mut Toasts) {
        self.run_edit(EditOp::Reset(groups), toasts);
    }

    pub fn can_undo(&self) -> bool {
        self.session.edit().is_some_and(EditState::can_undo)
    }

    pub fn can_redo(&self) -> bool {
        self.session.edit().is_some_and(EditState::can_redo)
    }

    /// The photo's RAW develop settings changed: it must be opened again (the library asks).
    pub fn wants_reopen(&self) -> bool {
        self.reopen && self.is_ready()
    }

    fn run_edit(&mut self, op: EditOp, toasts: &mut Toasts) {
        let damaged = matches!(&self.state, State::Ready(o, _) if o.sidecar_error.is_some());
        // Auto-sync: what a gesture (or a reset) changes goes to the other selected photos;
        // undo and redo stay on this one.
        let syncing = self.auto_sync
            && self.sync_others > 0
            && matches!(op, EditOp::Commit | EditOp::Reset(_));
        let before = syncing
            .then(|| {
                self.session
                    .edit()
                    .map(|e| (e.photo_id, e.committed.clone()))
            })
            .flatten();
        let committed = self
            .session
            .run(op, self.photo_path.as_deref(), damaged, toasts);
        if committed
            && let (Some((id, before)), Some(edit)) = (before, self.session.edit())
            && edit.photo_id == id
            && let Some(patch) = crate::sync::changes(&before, &edit.committed)
        {
            match &mut self.sync_out {
                Some((photo, waiting)) if *photo == id => crate::sync::merge(waiting, patch),
                out => *out = Some((id, patch)),
            }
        }
        if op != EditOp::Commit {
            self.settings_changed();
            // The points changed under the curve editor's selection (and the spot tool's).
            self.panels.curve.clear_selection();
            self.tools.clear_selection();
        }
        if committed {
            self.check_develop_change();
        }
    }

    /// A RAW whose develop settings (profile, highlights) no longer match what it was
    /// decoded with is opened again.
    fn check_develop_change(&mut self) {
        if let (State::Ready(opened, _), Some(edit)) = (&self.state, self.session.edit())
            && opened.kind == Kind::Raw
            && DevelopSettings::from_settings(&edit.settings) != opened.develop
        {
            self.reopen = true;
        }
    }

    /// Puts due sidecar writes on the `Io` lane and wakes the UI when the next falls due.
    /// Retries a commit the catalogue refused as busy.
    pub fn tick(&mut self, ctx: &egui::Context, jobs: &Jobs, toasts: &mut Toasts) {
        let now = Instant::now();
        if self.session.retry_due(now) {
            self.commit(toasts);
        }
        if let Some(at) = self.session.retry_at() {
            ctx.request_repaint_after(at.saturating_duration_since(now));
        }
        if let Some(due) = self.session.pump(now, jobs) {
            ctx.request_repaint_after(due.saturating_duration_since(now));
        }
        if let Some(due) = self.session.edited_due() {
            ctx.request_repaint_after(due.saturating_duration_since(now));
        }
    }

    /// The photos whose edits changed here and whose thumbnails and rows the library should
    /// refresh now: the open photo a quiet second after its last change, a photo that was
    /// left at once.
    pub fn take_edited(&mut self, now: Instant) -> Vec<i64> {
        let open = self.photo_id();
        self.session.take_edited(open, now)
    }

    /// "Import .cube…" was clicked since the last call.
    pub fn take_lut_import(&mut self) -> bool {
        std::mem::take(&mut self.lut_import_asked)
    }

    /// Auto-sync on or off.
    pub fn set_auto_sync(&mut self, on: bool) {
        self.auto_sync = on;
    }

    pub fn auto_sync(&self) -> bool {
        self.auto_sync
    }

    /// How many other photos are selected (Auto-sync's targets), each frame.
    pub fn set_sync_others(&mut self, n: usize) {
        self.sync_others = n;
    }

    /// The change waiting for Auto-sync: the photo it was made on and the keys.
    pub fn take_sync(&mut self) -> Option<(i64, serde_json::Value)> {
        self.sync_out.take()
    }

    /// "Import .dcp…" was clicked since the last call.
    pub fn take_dcp_import(&mut self) -> bool {
        std::mem::take(&mut self.dcp_import_asked)
    }

    /// Puts the camera profile `name` on the open photo, one history step (the photo is
    /// developed again with it).
    pub fn choose_dcp(&mut self, name: String, toasts: &mut Toasts) {
        if self.session.edit().is_none() {
            return;
        }
        self.commit(toasts);
        self.edit(|s| lf_settings::DevelopSettings::set_dcp(s, Some(&name)));
        self.commit(toasts);
    }

    /// Puts the LUT `name` on the open photo (keeping the amount it had), one history step.
    pub fn choose_lut(&mut self, name: String, toasts: &mut Toasts) {
        use lf_settings::lut::LutChoice;
        if self.session.edit().is_none() {
            return;
        }
        self.commit(toasts);
        self.edit(|s| {
            let amount = LutChoice::from_settings(s).map_or(100.0, |c| c.amount);
            LutChoice::write_into(Some(&LutChoice { name, amount }), s);
        });
        self.commit(toasts);
    }

    /// The `Io` lane wrote the waiting sidecars.
    pub fn sidecar_written(&mut self, result: Result<(), String>, toasts: &mut Toasts) {
        self.session.sidecar_written(result, toasts);
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

    /// Whether the preview is soft-proofed.
    pub fn proofing(&self) -> bool {
        self.proof.is_some()
    }

    /// Soft-proofs the preview through the ICC profile at `profile` (`None` stops).
    pub fn set_proof(&mut self, profile: Option<PathBuf>) {
        self.proof = profile;
        self.proof_key = None;
        self.dirty = true;
    }

    /// Builds the proof LUT (the photo's look, then the profile) when the profile or the look
    /// changed, and registers it for the renderer; a profile that fails stops proofing.
    fn refresh_proof(&mut self, settings: &lf_settings::EditSettings, toasts: &mut Toasts) {
        let Some(profile) = self.proof.clone() else {
            return;
        };
        let look = lf_settings::lut::LutChoice::from_settings(settings);
        let key = (
            profile.clone(),
            look.as_ref().map(|c| c.name.clone()),
            look.as_ref()
                .map_or(0, |c| (c.amount * 1000.0).round() as i64),
        );
        if self.proof_key.as_ref() == Some(&key) {
            return;
        }
        let lut = look
            .as_ref()
            .and_then(|c| lf_pipeline::lut::get(&c.name).map(|l| (l, c.amount as f32)));
        match lf_catalog::colour::proof_lut(&profile, lut.as_ref().map(|(l, a)| (&**l, *a))) {
            Ok(proof) => {
                lf_pipeline::lut::register(lf_pipeline::lut::SOFT_PROOF, proof);
                self.proof_key = Some(key);
                self.dirty = true;
            }
            Err(e) => {
                toasts.push(Level::Warning, e.to_string());
                self.proof = None;
                self.proof_key = None;
            }
        }
    }

    /// "Apply lens profile" asked since the last call: what the library's job needs from the
    /// open photo (its EXIF lens, maker, focal length and aperture, and its shape). A photo
    /// whose EXIF does not say is answered with a toast here.
    pub fn take_lens_profile_request(
        &mut self,
        toasts: &mut Toasts,
    ) -> Option<crate::lens_profiles::Query> {
        if !std::mem::take(&mut self.lens_profile_asked) {
            return None;
        }
        let State::Ready(photo, id) = &self.state else {
            return None;
        };
        let e = &photo.exif;
        match (&e.lens, e.focal_length) {
            (Some(lens), Some(focal)) if focal > 0.0 => Some(crate::lens_profiles::Query {
                photo_id: *id,
                lens: lens.clone(),
                maker: e.make.clone(),
                focal: f64::from(focal),
                aperture: e.f_number.map(f64::from),
                aspect: f64::from(photo.full_width) / f64::from(photo.full_height.max(1)),
            }),
            _ => {
                toasts.push(Level::Info, crate::lens_profiles::NO_LENS);
                None
            }
        }
    }

    /// A lens profile's sliders for photo `id`, applied (perspective kept) as one step.
    pub fn apply_lens_profile(
        &mut self,
        id: i64,
        profile: lf_settings::lens::Lens,
        toasts: &mut Toasts,
    ) -> bool {
        if self.photo_id() != Some(id) {
            return false;
        }
        self.commit(toasts);
        self.edit(|s| {
            let now = lf_settings::lens::Lens::from_settings(s);
            lf_settings::lens::Lens {
                vertical: now.vertical,
                horizontal: now.horizontal,
                ..profile
            }
            .write_into(s);
        });
        self.commit(toasts);
        true
    }

    /// Whether the AI buttons may be used.
    pub fn set_local_ai(&mut self, allowed: bool) {
        self.local_ai = allowed;
    }

    /// An AI mask asked for since the last call (a button, or a click with Select object):
    /// the tool to run.
    pub fn take_ai_mask_request(&mut self, busy: bool) -> Option<crate::ai_job::AiTool> {
        // A preset's adaptive masks, one at a time while no other AI job runs.
        if !busy
            && self.adaptive.waiting.is_none()
            && self.is_ready()
            && let Some(next) = self.adaptive.queue.pop_front()
        {
            let target = extras::target_of(&next.what)?;
            self.adaptive.waiting = Some(next);
            return Some(crate::ai_job::AiTool::Mask { target });
        }
        match &mut self.tools.active {
            Some(tools::Active::Masks(tool)) => tool
                .ai_request
                .take()
                .map(|target| crate::ai_job::AiTool::Mask { target })
                .or_else(|| {
                    tool.object_request
                        .take()
                        .map(|(u, v)| crate::ai_job::AiTool::ObjectMask {
                            u: u as f32,
                            v: v as f32,
                        })
                }),
            _ => None,
        }
    }

    /// An AI job ended: an adaptive mask it was finding and did not bring (nothing found, or
    /// it failed) is let go, so the rest of the preset's masks carry on.
    pub fn ai_job_ended(&mut self) {
        self.adaptive.waiting = None;
    }

    /// Adds an AI mask (found for the open photo) as a new mask, one history step, picked in
    /// the Masks tool when it is open.
    pub fn add_ai_mask(&mut self, shape: lf_settings::masks::Shape, toasts: &mut Toasts) {
        use lf_settings::masks::{Local, Mask, Masks, Range, Shape};
        // A preset's adaptive mask brings its own settings.
        let template = match (&shape, &self.adaptive.waiting) {
            (Shape::Image { what, .. }, Some(t)) if *what == t.what => self.adaptive.waiting.take(),
            _ => None,
        };
        let mask = match template {
            Some(t) => t.with_shape(shape),
            None => Mask {
                shape,
                invert: false,
                opacity: 100.0,
                hidden: false,
                range: Range::All,
                local: Local::default(),
            },
        };
        self.commit(toasts);
        let mut refused = None;
        let mut picked = None;
        self.edit(|s| {
            let mut list = Masks::from_settings(s);
            if list.0.len() >= lf_settings::masks::MAX_MASKS {
                refused = Some(tools::masks::FULL);
                return;
            }
            list.0.push(mask);
            picked = Some(list.0.len() - 1);
            list.write_into(s);
        });
        if let Some(problem) = refused {
            toasts.push(Level::Warning, problem);
            return;
        }
        if let (Some(tools::Active::Masks(tool)), Some(i)) = (&mut self.tools.active, picked) {
            tool.selected = Some(i);
        }
        self.commit(toasts);
    }

    /// Whether the preview shows clipping.
    pub fn clipping(&self) -> bool {
        self.clipping
    }

    /// View → Clipping (`J`).
    pub fn toggle_clipping(&mut self) {
        self.clipping = !self.clipping;
    }

    /// Whether the preview shows focus peaking.
    pub fn peaking(&self) -> bool {
        self.peaking
    }

    /// View → Focus peaking (`Shift+J`).
    pub fn toggle_peaking(&mut self) {
        self.peaking = !self.peaking;
    }

    /// Whether the preview shows the photo as edges (visualize spots).
    pub fn spots_view(&self) -> bool {
        self.spots_view
    }

    pub fn side_by_side(&self) -> bool {
        self.side_by_side
    }

    /// View → Before/After side by side (`Y`).
    pub fn toggle_side_by_side(&mut self) {
        self.side_by_side = !self.side_by_side;
    }

    /// A quarter turn of the open photo, one history step.
    pub fn turn(&mut self, dir: lf_settings::geometry::TurnDir, toasts: &mut Toasts) {
        if !self.is_ready() {
            return;
        }
        self.commit(toasts);
        self.edit(|s| tools::crop::turn(s, dir));
        self.commit(toasts);
    }

    pub fn info_overlay(&self) -> bool {
        self.info_overlay
    }

    /// View → Info (`I`): the file's facts over the preview.
    pub fn toggle_info_overlay(&mut self) {
        self.info_overlay = !self.info_overlay;
    }

    /// View → Visualize spots.
    pub fn toggle_spots_view(&mut self) {
        self.spots_view = !self.spots_view;
    }

    /// View → Before/After (or the side panel's "Before").
    pub fn toggle_before(&mut self) {
        self.before_toggled = !self.before_toggled;
    }

    /// The `\` key is held (the app reads it each frame).
    pub fn set_before_held(&mut self, held: bool) {
        self.before_held = held;
    }

    /// View → Fit, View → 100 % or `Space` (fit ⇄ 100 %).
    pub fn zoom(&mut self, action: Action) {
        self.zoom_asked = Some(action);
    }

    /// Develop → Reset all: every setting back to its default (one history step, so
    /// undo brings the edit back).
    pub fn reset_all(&mut self, toasts: &mut Toasts) {
        if self.session.edit().is_some() {
            self.leave_tool(toasts);
            self.replace_settings(lf_settings::EditSettings::default_settings(), toasts);
            self.panels.curve.clear_selection();
            self.tools.clear_selection();
            self.check_develop_change();
        }
    }

    /// Starts this view's generations after `from` (a library restarted on another
    /// catalogue): an open answer meant for the old view is never taken for a new one.
    pub fn carry_generation(&mut self, from: u64) {
        self.latest.fetch_max(from, Ordering::SeqCst);
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
        toasts: &mut Toasts,
    ) {
        let same = self.photo_id() == Some(photo.id);
        if same && !matches!(self.state, State::Failed(..)) && !self.wants_reopen() {
            return;
        }
        if !same {
            // A preset's adaptive masks were for the photo left.
            self.adaptive = extras::AdaptiveQueue::default();
        }
        if self.check_lost() {
            return;
        }
        // Reopening the shown photo (its RAW develop settings changed) keeps it on screen,
        // with its edits and zoom, until the new decode arrives.
        let reopening = same && self.wants_reopen();
        self.reopen = false;
        if !reopening {
            self.finish_edit(toasts);
            self.tools.active = None;
            self.session.clear();
            self.panels.curve.clear_selection();
            self.state = State::Opening(photo.id);
            self.preview = None;
            self.histogram = None;
            self.viewport = Viewport::default();
            self.before_toggled = false;
        }
        self.session.set_root(root);
        self.photo_path = Some(photo.path.clone());
        let generation = self.latest.fetch_add(1, Ordering::SeqCst) + 1;
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

    /// Leaves Develop: a gesture in progress is committed, the renderer and canvas are
    /// freed; queued opens are skipped.
    pub fn close(&mut self, toasts: &mut Toasts) {
        self.finish_edit(toasts);
        self.tools.active = None;
        self.session.clear();
        self.panels.curve.clear_selection();
        self.reopen = false;
        self.latest.fetch_add(1, Ordering::SeqCst);
        self.preview = None;
        self.last_drawn = None;
        self.last_before = None;
        self.navigator_drawn = None;
        self.readout_img = None;
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
                let reopened = matches!(&self.state, State::Ready(_, id) if *id == photo_id)
                    && self.session.edit().is_some_and(|e| e.photo_id == photo_id);
                if !reopened {
                    self.session.start(photo_id, photo.settings.clone());
                    self.panels.curve.clear_selection();
                    self.viewport = Viewport::default();
                    self.histogram = None;
                    if photo.sidecar_error.is_some() {
                        let path = self.photo_path.clone().unwrap_or_default();
                        self.session.warn_damaged(photo_id, &photo.file_name, &path);
                    }
                }
                self.state = State::Ready(photo, photo_id);
                if reopened {
                    // The settings may have changed again while it decoded (Linear, then
                    // Camera before the answer): a decode that no longer matches reopens.
                    self.check_develop_change();
                }
                self.last_drawn = None;
                self.last_before = None;
                self.navigator_drawn = None;
                self.readout_img = None;
                self.readout_img = None;
                self.navigator_drawn = None;
                self.readout_img = None;
                self.readout_img = None;
                self.last_before = None;
                self.navigator_drawn = None;
                self.readout_img = None;
                self.readout_img = None;
                self.navigator_drawn = None;
                self.readout_img = None;
                self.readout_img = None;
                self.last_histogram = None;
                self.settings_changed();
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
        self.last_before = None;
        self.navigator_drawn = None;
        self.readout_img = None;
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

    /// The side panel: histogram, info strip, the damaged-sidecar warning, the Before button
    /// and the adjust panels.
    pub fn side_ui(&mut self, ui: &mut egui::Ui, photo: Option<&PhotoRow>, toasts: &mut Toasts) {
        ui.add_space(8.0);
        histogram::show(ui, self.histogram.as_ref());
        let value = self
            .readout_img
            .as_ref()
            .zip(self.readout_at)
            .and_then(|(img, at)| readout::value_at(img, at));
        // Always a line, so the panel below does not jump as the pointer comes and goes.
        ui.label(
            RichText::new(value.map(readout::text).unwrap_or_default())
                .monospace()
                .small()
                .color(theme::MUTED),
        );
        ui.add_space(4.0);
        if self.is_ready() {
            self.navigator_ui(ui, toasts);
        }
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
        } else if let Some(photo) = photo {
            ui.label(RichText::new(open::file_name(&photo.path)).strong());
            return;
        } else {
            return;
        }
        self.damaged_ui(ui, toasts);
        if self.sync_others > 0 {
            ui.add_space(4.0);
            let label = if self.sync_others == 1 {
                "Auto-sync the other selected photo".to_owned()
            } else {
                format!(
                    "Auto-sync the {} other selected photos",
                    crate::catalog_ext::count(self.sync_others)
                )
            };
            ui.checkbox(&mut self.auto_sync, label)
                .on_hover_text(AUTO_SYNC_HINT);
        }
        ui.add_space(8.0);
        let before = self.before();
        if ui.selectable_label(before, BEFORE).clicked() {
            self.toggle_before();
        }
        ui.separator();
        self.tool_ui(ui, toasts);
        // Looks, presets and versions scroll with the adjust panels.
        egui::ScrollArea::vertical()
            .id_salt("adjust-panels")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.extras_ui(ui, toasts);
                self.panels_ui(ui, toasts);
            });
    }

    /// The damaged-sidecar warning, once per photo per session, until dismissed.
    fn damaged_ui(&mut self, ui: &mut egui::Ui, toasts: &mut Toasts) {
        let Some(damaged) = self.session.damaged().cloned() else {
            return;
        };
        if self.photo_id() != Some(damaged.photo_id) {
            return;
        }
        ui.add_space(8.0);
        let mut dismiss = false;
        egui::Frame::new()
            .fill(theme::SURFACE_2)
            .stroke(egui::Stroke::new(1.0, theme::WARNING))
            .corner_radius(4)
            .inner_margin(8)
            .show(ui, |ui| {
                ui.label(RichText::new(damaged_text(&damaged.name)).color(theme::WARNING));
                ui.horizontal(|ui| {
                    if ui.button(SHOW_FILE).clicked()
                        && let Err(e) = crate::platform::show_in_folder(&damaged.sidecar)
                    {
                        toasts.push(Level::Error, format!("The file could not be shown. {e}"));
                    }
                    if ui.button("Dismiss").clicked() {
                        dismiss = true;
                    }
                });
            });
        if dismiss {
            self.session.dismiss_damaged();
        }
    }

    /// The adjust panels for the open photo; a gesture's end commits it.
    fn panels_ui(&mut self, ui: &mut egui::Ui, toasts: &mut Toasts) {
        let (State::Ready(opened, _), Some(edit)) = (&self.state, self.session.edit_mut()) else {
            return;
        };
        let luts = lf_pipeline::lut::names();
        let profiles = lf_raw::dcp::names();
        let ctx = PanelContext {
            profiles: &profiles,
            as_shot_kelvin: match &self.state {
                State::Ready(photo, _) => photo.as_shot_kelvin,
                _ => None,
            },
            luts: &luts,
            raw: opened.kind == Kind::Raw,
            histogram: self.histogram.as_ref(),
            sample: Some(&opened.sample),
            picking_wb: self.tools.kind() == Some(ToolKind::WhiteBalance),
            targeting_hsl: self.tools.kind() == Some(ToolKind::Targeted),
            picking_point: self.tools.kind() == Some(ToolKind::PointColour),
        };
        let panels = &mut self.panels;
        let mut out = PanelOutput::default();
        edit.set(|s| out = panels::show(ui, panels, s, &ctx));
        if out.changed {
            self.settings_changed();
        }
        if let Some(groups) = out.reset {
            self.reset_groups(groups, toasts);
        } else if out.commit {
            self.commit(toasts);
        }
        if out.pick_wb {
            self.toggle_tool(ToolKind::WhiteBalance, toasts);
        }
        if out.target_hsl {
            self.toggle_tool(ToolKind::Targeted, toasts);
        }
        if out.pick_point {
            self.toggle_tool(ToolKind::PointColour, toasts);
        }
        // The targeted adjustment follows the property the panel shows.
        if let Some(tools::Active::Targeted(t)) = &mut self.tools.active {
            t.mode = self.panels.hsl_mode;
        }
        self.lut_import_asked |= out.import_lut;
        self.dcp_import_asked |= out.import_dcp;
        self.lens_profile_asked |= out.lens_profile;
    }

    /// Develop → Auto tone (`Ctrl+U`): exposure and the tone sliders from the photo, one
    /// history step.
    pub fn auto_tone(&mut self, toasts: &mut Toasts) {
        let (Some(opened), Some(edit)) = (self.opened(), self.session.edit()) else {
            return;
        };
        if let Some(tone) = auto::auto_tone(&opened.sample, &edit.settings) {
            self.commit(toasts);
            self.edit(|s| tone.apply(s));
            self.commit(toasts);
        }
    }

    /// Develop → Auto WB (`Ctrl+Shift+U`), one history step.
    pub fn auto_white_balance(&mut self, toasts: &mut Toasts) {
        let Some(found) = self.opened().and_then(|o| auto::auto_wb(&o.sample)) else {
            toasts.push(Level::Info, auto::NO_NEUTRAL);
            return;
        };
        self.commit(toasts);
        self.edit(|s| (s.temperature, s.tint) = found);
        self.commit(toasts);
    }

    /// The preview area: draws the photo, handles zoom, pan and before/after.
    pub fn preview_ui(&mut self, ui: &mut egui::Ui, toasts: &mut Toasts) {
        self.check_lost();
        let (whole, response) =
            ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        // Side by side: the photo as it was on the left, the edit (and everything else) on
        // the right.
        let (rect, before_rect) = if self.side_by_side && !self.before() {
            let (l, r) = crate::library::reference::split(whole);
            (r, Some(l))
        } else {
            (whole, None)
        };
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
        let draw_crop = self.draw_crop();
        let output = match &self.state {
            State::Ready(photo, _) => {
                let settings = self.session.edit().map_or(&photo.settings, |e| &e.settings);
                let mut geometry = Geometry::from(settings);
                if let Some(crop) = draw_crop {
                    geometry.crop = crop;
                }
                let r = &photo.renderer;
                let (w, h) = geometry::output_size(r.full_width, r.full_height, &geometry);
                (f64::from(w), f64::from(h))
            }
            _ => return,
        };
        let before = self.before();
        let map = |viewport: &Viewport| {
            let placement = viewport.placement(output, panel);
            tools::ScreenMap {
                shown: tools::shown_rect(rect, placement.rect, ppp),
                view: placement.view,
            }
        };
        let tool = if before {
            tools::ToolInput::default()
        } else {
            self.tool_input(ui, &response, map(&self.viewport), output.0, toasts)
        };
        self.interact(ui, rect, &response, output, panel, tool);

        let Some(render_state) = self.render_state.clone() else {
            centred(ui, rect, |ui| {
                ui.label(RichText::new(NO_PREVIEW).color(theme::MUTED));
            });
            self.paint_info(ui, rect);
            return;
        };
        let placement = self.viewport.placement(output, panel);
        self.last_view = placement.view;
        // The photo point under the pointer, for the colour readout (not over the photo as
        // it was, nor the crop tool's whole frame).
        self.readout_at = response
            .hover_pos()
            .filter(|p| rect.contains(*p) && !before && draw_crop.is_none())
            .map(|p| {
                let c = (p - rect.min) / rect.size();
                self.viewport
                    .output_at((f64::from(c.x), f64::from(c.y)), output, panel)
            })
            .filter(|(u, v)| (0.0..=1.0).contains(u) && (0.0..=1.0).contains(v));
        let (_, _, rw, rh) = placement.rect;
        let size = preview::canvas_size(&self.gpu, rw, rh);
        if let State::Ready(photo, _) = &self.state {
            let settings = self
                .session
                .edit()
                .map_or_else(|| photo.settings.clone(), |e| e.settings.clone());
            self.refresh_proof(&settings, toasts);
        }
        let key = DrawKey {
            size,
            view: placement.view,
            before,
            clipping: self.clipping,
            peaking: self.peaking,
            spots_view: self.spots_view,
            proof: self.proof.is_some(),
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
                    crop: draw_crop,
                    view: placement.view,
                    // Never over the photo as it was.
                    clipping: self.clipping && !before,
                    peaking: self.peaking && !before,
                    spots_view: self.spots_view && !before,
                    proof: self.proof.is_some() && !before,
                };
                let settings = self.session.edit().map_or(&photo.settings, |e| &e.settings);
                if let Err(e) = photo.renderer.draw(settings, &opts, canvas) {
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
        let shown = tools::shown_rect(rect, placement.rect, ppp);
        ui.painter().image(
            texture,
            shown,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
        if !before {
            self.tool_paint(ui, rect, map(&self.viewport), output.0);
        }
        if let Some(left) = before_rect {
            self.paint_before_half(ui, left, toasts);
            label(ui, rect, AFTER);
            label(ui, left, BEFORE);
        }
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
        self.paint_info(ui, rect);
        self.update_histogram(ui.ctx(), toasts);
    }

    /// The left half of side by side: the photo as it was, fitted (its canvas drawn again only
    /// when its size or view changes; edits do not change it).
    fn paint_before_half(&mut self, ui: &egui::Ui, left: egui::Rect, toasts: &mut Toasts) {
        let ppp = ui.ctx().pixels_per_point();
        let panel = (
            f64::from(left.width() * ppp),
            f64::from(left.height() * ppp),
        );
        let (Some(render_state), State::Ready(photo, _)) =
            (self.render_state.clone(), &mut self.state)
        else {
            return;
        };
        if panel.0 < 1.0 || panel.1 < 1.0 {
            return;
        }
        let output = (
            f64::from(photo.renderer.full_width),
            f64::from(photo.renderer.full_height),
        );
        let placement = Viewport::default().placement(output, panel);
        let (_, _, rw, rh) = placement.rect;
        let size = preview::canvas_size(&self.gpu, rw, rh);
        let preview = self
            .preview_before
            .get_or_insert_with(|| Preview::new(render_state));
        let (canvas, texture, fresh) = match preview.ensure(&self.gpu, size.0, size.1) {
            Ok(made) => made,
            Err(e) => {
                self.preview_before = None;
                self.renderer_error("The before half could not be made", e, toasts);
                return;
            }
        };
        let key = (size, placement.view);
        if fresh || self.last_before != Some(key) {
            let opts = DrawOptions {
                before: true,
                view: placement.view,
                ..DrawOptions::default()
            };
            let settings = self.session.edit().map_or(&photo.settings, |e| &e.settings);
            if let Err(e) = photo.renderer.draw(settings, &opts, canvas) {
                self.renderer_error("The before half could not be drawn", e, toasts);
                return;
            }
            self.last_before = Some(key);
        }
        ui.painter().image(
            texture,
            tools::shown_rect(left, placement.rect, ppp),
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }

    /// The info overlay (`I`), bottom left over the preview.
    fn paint_info(&self, ui: &egui::Ui, rect: egui::Rect) {
        if self.info_overlay
            && let State::Ready(opened, _) = &self.state
        {
            let text = info::overlay_lines(
                &opened.file_name,
                opened.kind,
                opened.full_width,
                opened.full_height,
                &opened.exif,
            )
            .join("\n");
            let galley = ui.painter().layout_no_wrap(
                text.clone(),
                egui::FontId::proportional(13.0),
                theme::INK,
            );
            let at = rect.left_bottom() + egui::vec2(12.0, -12.0 - galley.size().y);
            let back = egui::Rect::from_min_size(at, galley.size()).expand(6.0);
            ui.painter()
                .rect_filled(back, 4.0, theme::SURFACE.gamma_multiply(0.85));
            ui.painter().galley(at, galley, theme::INK);
            // Readable by screen readers (and tests) as one label.
            let info = ui.interact(back, ui.scope_id().with("info-overlay"), Sense::hover());
            info.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &text));
        }
    }

    /// Scroll zooms at the cursor, drag pans, double-click (and `Space`, through
    /// [`Develop::zoom`]) toggles fit and 100 %. An open tool owns drags (and the wheel over a
    /// spot).
    fn interact(
        &mut self,
        ui: &egui::Ui,
        rect: egui::Rect,
        response: &egui::Response,
        output: (f64, f64),
        panel: (f64, f64),
        tool: tools::ToolInput,
    ) {
        let ppp = ui.ctx().pixels_per_point();
        match self.zoom_asked.take() {
            Some(Action::Fit) => self.viewport.zoom = Zoom::Fit,
            Some(Action::ActualSize) => self.viewport.zoom = Zoom::Scale(1.0),
            Some(_) => self.viewport.toggle_fit_100(output, panel),
            None => {}
        }
        if response.double_clicked() && !tool.owns_drag {
            self.viewport.toggle_fit_100(output, panel);
        }
        if let Some(pos) = response.hover_pos()
            && !tool.took_wheel
        {
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
        if response.dragged() && !tool.owns_drag {
            let d = response.drag_delta() * ppp;
            self.viewport
                .pan(f64::from(d.x), f64::from(d.y), output, panel);
        }
    }

    /// Remakes the histogram 200 ms after the last change, once per change.
    fn update_histogram(&mut self, ctx: &egui::Context, toasts: &mut Toasts) {
        // The readout's picture, once the pointer is over the photo and none is current.
        if self.readout_at.is_some()
            && self.readout_img.is_none()
            && let State::Ready(photo, _) = &mut self.state
        {
            let settings = self.session.edit().map_or(&photo.settings, |e| &e.settings);
            match photo.renderer.pixels(settings, readout::EDGE) {
                Ok(img) => {
                    self.readout_img = Some(img);
                    ctx.request_repaint();
                }
                Err(e) => self.renderer_error("The colour readout could not be made", e, toasts),
            }
        }
        let now = Instant::now();
        if !Throttle::should_run(self.last_change, self.last_histogram, now) {
            if self.last_histogram.is_none_or(|run| run < self.last_change) {
                let due = (self.last_change + histogram::DELAY).saturating_duration_since(now);
                ctx.request_repaint_after(due);
            }
            return;
        }
        self.last_histogram = Some(now);
        // The edit changed: the readout's picture is made again (while the pointer is over the
        // photo).
        self.readout_img = None;
        let State::Ready(photo, _) = &mut self.state else {
            return;
        };
        let settings = self.session.edit().map_or(&photo.settings, |e| &e.settings);
        match photo.renderer.histogram(settings) {
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

/// A small label at the top left of `rect` ("Before", "After").
fn label(ui: &egui::Ui, rect: egui::Rect, text: &str) {
    let at = rect.left_top() + egui::vec2(12.0, 12.0);
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        egui::FontId::proportional(14.0),
        theme::INK,
    );
    let back = egui::Rect::from_min_size(at, galley.size()).expand(6.0);
    ui.painter().rect_filled(back, 4.0, theme::SURFACE);
    ui.painter().galley(at, galley, theme::INK);
}
