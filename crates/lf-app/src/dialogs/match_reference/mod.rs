//! "Match a reference…", as the web's `MatchReferencePanel.vue` with its apply step from
//! `BatchEditDialog.vue`: choose a reference picture, see what it costs, fit one grade for
//! every selected photo, preview it at a strength, and apply it.
//!
//! * [`samples`] picks up to 24 of the selected photos and tiles them into a contact sheet.
//! * [`steps`] measures, asks the AI (only when allowed, after a fresh check), calibrates.
//! * [`preview`] is the before/after of one sampled photo.
//!
//! The fit runs on the `Open` lane (it opens photos and renders the sheet on the device) and
//! reports progress; closing the dialog stops it at the next step. Applying writes each photo
//! through the batch's per-photo path (one `set_settings` and its sidecar).

pub mod preview;
pub mod samples;
pub mod steps;

use crate::catalog_ext::count;
use crate::clipboard::{Recipe, picked};
use crate::develop::debounce::SidecarSaver;
use crate::dialogs::batch;
use crate::jobs::Msg;
use crate::theme;
use eframe::egui::{self, RichText};
use image::RgbaImage;
use lf_account::entitlement::Decision;
use lf_catalog::{Catalog, PhotoRow};
use lf_grade::sheet::{AI_EDGE, MEASURE_EDGE, TILE_EDGE, pick_samples, scaled};
use lf_grade::{apply_strength, exposure_offset};
use lf_settings::EditSettings;
use lf_settings::groups::DEFAULT_COPY_GROUPS;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use steps::{AiLink, MatchCtx, MatchError, Outcome, SheetRender};

pub const MENU: &str = "Match a reference…";
pub const TITLE: &str = "Match a reference";
pub const CHOOSE: &str = "Choose a reference picture";
pub const OR_PHOTO: &str = "Or one of the selected photos";
pub const REVIEW: &str = "Let AI check the result (1 more request)";
pub const SENT: &str = "The reference and a small contact sheet of your photos are sent to the AI service to read the look. LensFlow doesn't keep them; the AI service's data policy applies.";
pub const READ: &str = "Read the look";
pub const MEASURE: &str = "Measure the look";
pub const ALL_EDITED: &str = "Every selected photo already has an edit: the grade was fitted to the photos as they were taken, and applying it replaces those edits.";
pub const NO_CATALOGUE: &str =
    "The catalogue is not open, so the grade cannot be applied. Open the catalogue and try again.";
pub const HOLD: &str = "Press and hold the picture to see it before.";
pub const STRENGTH: &str = "Strength";
pub const EXPOSURE: &str = "Match exposure per photo";
pub const EXPOSURE_NOTE: &str =
    "Evens out brightness frame by frame; the colour grade stays identical.";
pub const PRESET: &str = "Save as preset (optional)";
pub const PRESET_HINT: &str = "e.g. Client's matte look";
pub const CANCEL: &str = "Cancel";
pub const STOP: &str = "Stop";
pub const DONE: &str = "Done";
pub const IMAGES: &[&str] = &["jpg", "jpeg", "png", "tif", "tiff", "webp"];

/// "12 of your 30 selected photos will be read to work out one grade for all of them."
pub fn sample_text(samples: usize, selected: usize) -> String {
    let noun = if selected == 1 { "photo" } else { "photos" };
    format!(
        "{} of your {} selected {noun} will be read to work out one grade for all of them.",
        count(samples),
        count(selected)
    )
}

pub fn reading_text(done: usize, total: usize) -> String {
    format!("Reading your photos… {done} of {total}")
}

pub fn skipped_text(n: usize) -> String {
    if n == 1 {
        "1 photo couldn't be read and was skipped.".to_owned()
    } else {
        format!("{n} photos couldn't be read and were skipped.")
    }
}

/// Run and picker ids, unique for the process: an answer for a dialog closed meanwhile never
/// lands in the next one.
static IDS: AtomicU64 = AtomicU64::new(1);

/// A new run or picker id.
pub fn next_id() -> u64 {
    IDS.fetch_add(1, Ordering::Relaxed)
}

/// What the cost line needs from the gate's last answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AiGate {
    pub allowed: bool,
    /// `None`: unlimited.
    pub remaining: Option<u64>,
    /// None left this month.
    pub used_up: bool,
}

impl AiGate {
    pub fn from(decision: Option<&Decision>) -> AiGate {
        let Some(d) = decision else {
            return AiGate::default();
        };
        let remaining = steps::remaining(d);
        AiGate {
            allowed: steps::ai_usable(d),
            remaining,
            used_up: d.ai_allowed() && remaining == Some(0),
        }
    }
}

/// The reference picture: a file anywhere, or a catalogued photo.
#[derive(Debug, Clone, PartialEq)]
pub enum Reference {
    File(PathBuf),
    Photo { id: i64, path: PathBuf },
}

impl Reference {
    fn path(&self) -> &Path {
        match self {
            Reference::File(p) | Reference::Photo { path: p, .. } => p,
        }
    }
}

/// The fitted grade with what the preview and the apply step need.
pub struct Fitted {
    pub outcome: Outcome,
    /// Renders the sheet (the preview's after).
    pub renderer: Box<dyn SheetRender>,
    pub sheet: RgbaImage,
    pub tiles: usize,
    /// Each sampled photo's median luma, unedited.
    pub medians: HashMap<i64, f64>,
    pub all_edited: bool,
    pub skipped: usize,
}

impl std::fmt::Debug for Fitted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fitted")
            .field("outcome", &self.outcome)
            .field("tiles", &self.tiles)
            .field("all_edited", &self.all_edited)
            .field("skipped", &self.skipped)
            .finish_non_exhaustive()
    }
}

/// How an apply went.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ApplyReport {
    pub total: usize,
    pub done: usize,
    pub edited: Vec<i64>,
    pub failed: Vec<(i64, String)>,
}

/// The match's messages (`Msg::Match`). `run` ties an answer to the fit or apply that asked;
/// older ones are dropped.
#[derive(Debug)]
pub enum MatchStep {
    /// The reference file picker answered (`None`: cancelled).
    Picked {
        ticket: u64,
        path: Option<PathBuf>,
    },
    Progress {
        run: u64,
        text: String,
    },
    Fitted {
        run: u64,
        result: Result<Box<Fitted>, MatchError>,
    },
    Applying {
        run: u64,
        done: usize,
        total: usize,
    },
    Applied {
        run: u64,
        report: ApplyReport,
    },
    /// The server signed this device out during a fit (asked in this generation): the
    /// session goes back to sign-in.
    SignedOut(crate::account::Generation),
}

/// One fit's input.
pub struct FitRequest {
    pub run: u64,
    pub photos: Vec<PhotoRow>,
    pub reference: Reference,
    pub ai: Option<AiLink>,
    pub review: bool,
    pub cancel: Arc<AtomicBool>,
}

/// Opens a file rendered with default settings at a long edge (the app: `open_cached` at
/// `Quality::Fast` / `AI_EDGE` and a temporary renderer).
pub type OpenFn<'a> = &'a dyn Fn(&Path, u32) -> Result<RgbaImage, String>;

/// The fit job: samples, reference, sheet renderer, then [`steps::run`]. Checks `cancel`
/// before each photo and between steps.
pub fn fit(
    req: FitRequest,
    open: OpenFn<'_>,
    make_renderer: impl FnOnce(&RgbaImage) -> Result<Box<dyn SheetRender>, String>,
    report: &dyn Fn(MatchStep),
) -> MatchStep {
    let run = req.run;
    let result = fit_inner(req, open, make_renderer, report).map(Box::new);
    MatchStep::Fitted { run, result }
}

fn fit_inner(
    mut req: FitRequest,
    open: OpenFn<'_>,
    make_renderer: impl FnOnce(&RgbaImage) -> Result<Box<dyn SheetRender>, String>,
    report: &dyn Fn(MatchStep),
) -> Result<Fitted, MatchError> {
    let run = req.run;
    let cancel = req.cancel.clone();
    let stopped = || cancel.load(Ordering::SeqCst);
    let total = pick_samples(&req.photos, |r| r.edited, samples::CAP)
        .0
        .len();
    progress(report, run, &reading_text(0, total));
    let read = std::cell::Cell::new(0);
    let built = samples::build(&req.photos, |path| {
        if stopped() {
            return Err(steps::STOPPED.to_owned());
        }
        // Read at 512 as the web reads them; `tiled` cover-fits each into its 256 px square.
        let bitmap = open(path, MEASURE_EDGE);
        read.set(read.get() + 1);
        progress(report, run, &reading_text(read.get(), total));
        bitmap
    });
    if stopped() {
        return Err(MatchError::Cancelled);
    }
    let sheet = built.map_err(MatchError::Failed)?;
    let reference = load_reference(&req.reference, open).map_err(MatchError::Failed)?;
    if stopped() {
        return Err(MatchError::Cancelled);
    }
    let tiled = sheet.tiled();
    let mut renderer = make_renderer(&tiled).map_err(MatchError::Failed)?;
    let outcome = steps::run(MatchCtx {
        reference: &reference,
        sheet: &tiled,
        renderer: renderer.as_mut(),
        ai: req.ai.as_mut(),
        review: req.review,
        cancel: &cancel,
        progress: &mut |text: &str| progress(report, run, text),
    })?;
    Ok(Fitted {
        outcome,
        renderer,
        tiles: sheet.bitmaps.len(),
        medians: sheet.medians(),
        all_edited: sheet.all_edited,
        skipped: sheet.skipped,
        sheet: tiled,
    })
}

/// The settings a photo with `current` gets: the grade's look groups laid on as a look (its
/// crop and spots stay its own, as the web's `applyLook`), plus `offset` EV of exposure.
pub fn final_settings(
    current: &EditSettings,
    grade: &EditSettings,
    offset: Option<f64>,
) -> EditSettings {
    let mut grade = grade.clone();
    if let Some(offset) = offset {
        grade.exposure = (grade.exposure + offset).clamp(-5.0, 5.0);
    }
    Recipe::Look(picked(&grade, &DEFAULT_COPY_GROUPS)).apply(current)
}

/// Writes the grade to each of `ids` in turn through the batch's per-photo path, with the
/// exposure `offset` each photo needs; stops before the next photo once `stop` is set.
pub fn apply(
    catalog: &Catalog,
    ids: &[i64],
    grade: &EditSettings,
    offset: &mut dyn FnMut(&PhotoRow) -> Result<Option<f64>, String>,
    saver: &SidecarSaver,
    stop: &AtomicBool,
    progress: &mut dyn FnMut(usize),
) -> ApplyReport {
    let mut report = ApplyReport {
        total: ids.len(),
        ..ApplyReport::default()
    };
    for &id in ids {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let written = batch::apply_with(catalog, id, saver, |row, current| {
            Ok(final_settings(current, grade, offset(row)?))
        });
        match written {
            Ok(()) => report.edited.push(id),
            Err(e) => {
                log::warn!("match: photo {id} could not be edited: {e}");
                report.failed.push((id, e));
            }
        }
        report.done += 1;
        progress(report.done);
    }
    report
}

/// What Apply asks for.
#[derive(Debug, Clone)]
pub struct ApplyPlan {
    /// The grade at the chosen strength.
    pub grade: EditSettings,
    /// The reference's median and the sampled photos' medians, when matching exposure.
    pub exposure: Option<(f64, HashMap<i64, f64>)>,
    /// A preset to save the grade as.
    pub preset: Option<String>,
}

/// The offset closure for an apply: the sampled photos' medians, others measured as they
/// come (opened and rendered with default settings).
pub fn offsets<'a>(
    exposure: &'a Option<(f64, HashMap<i64, f64>)>,
    open: OpenFn<'a>,
) -> impl FnMut(&PhotoRow) -> Result<Option<f64>, String> + 'a {
    move |row: &PhotoRow| {
        let Some((reference, medians)) = exposure else {
            return Ok(None);
        };
        let median = match medians.get(&row.id) {
            Some(m) => *m,
            None => samples::median(&open(&row.path, TILE_EDGE)?),
        };
        Ok(Some(exposure_offset(*reference, median)))
    }
}

/// What the dialog asks of the library this frame.
#[derive(Debug, Clone)]
pub enum MatchAction {
    None,
    /// Open the file picker.
    PickFile,
    /// Start the fit under the gate the dialog showed (the app supplies the AI link).
    Run(AiGate),
    Apply(Box<ApplyPlan>),
    /// The dialog is gone (a running fit or apply stops at its next step).
    Close,
}

enum Phase {
    Choose,
    Running(String),
    Ready(Box<Fitted>),
    Failed(String),
    Applying { done: usize, total: usize },
    Done(ApplyReport),
}

/// The open dialog.
pub struct MatchDialog {
    pub ids: Vec<i64>,
    photos: Vec<PhotoRow>,
    names: Vec<String>,
    pub reference: Option<Reference>,
    pub ticket: u64,
    pub review: bool,
    phase: Phase,
    /// Set when the dialog closes: the running fit or apply stops at its next step.
    pub cancel: Arc<AtomicBool>,
    pub run: u64,
    pub strength: f64,
    pub match_exposure: bool,
    pub preset_name: String,
    preview: preview::Preview,
    before: bool,
}

impl MatchDialog {
    /// For the selected photos, in display order.
    pub fn new(photos: Vec<PhotoRow>) -> MatchDialog {
        MatchDialog {
            ids: photos.iter().map(|p| p.id).collect(),
            names: photos
                .iter()
                .map(|p| crate::develop::open::file_name(&p.path))
                .collect(),
            photos,
            reference: None,
            ticket: 0,
            review: true,
            phase: Phase::Choose,
            cancel: Arc::new(AtomicBool::new(false)),
            run: 0,
            strength: 100.0,
            match_exposure: false,
            preset_name: String::new(),
            preview: preview::Preview::default(),
            before: false,
        }
    }

    /// How many photos a fit reads.
    pub fn sample_count(&self) -> usize {
        pick_samples(&self.photos, |r| r.edited, samples::CAP)
            .0
            .len()
    }

    pub fn running(&self) -> bool {
        matches!(self.phase, Phase::Running(_) | Phase::Applying { .. })
    }

    pub fn ready(&self) -> bool {
        matches!(self.phase, Phase::Ready(_))
    }

    /// The message shown (progress or failure).
    pub fn status(&self) -> Option<&str> {
        match &self.phase {
            Phase::Running(t) | Phase::Failed(t) => Some(t),
            _ => None,
        }
    }

    pub fn fitted(&self) -> Option<&Fitted> {
        match &self.phase {
            Phase::Ready(f) => Some(f),
            _ => None,
        }
    }

    /// The fit request for the reference chosen (a new run: older answers are dropped).
    ///
    /// The fit follows what the dialog showed when Run was pressed (`shown`): no AI when it
    /// said AI is off, even if the gate allows it now, and the review only when the cost line
    /// counted it. A gate that allowed AI but refuses it now is caught by the fresh check
    /// before each request.
    pub fn start(&mut self, ai: Option<AiLink>, shown: AiGate) -> Option<FitRequest> {
        let reference = self.reference.clone()?;
        self.run = next_id();
        self.cancel = Arc::new(AtomicBool::new(false));
        self.phase = Phase::Running(steps::READING.to_owned());
        self.preview = preview::Preview::default();
        Some(FitRequest {
            run: self.run,
            photos: self.photos.clone(),
            reference,
            ai: ai.filter(|_| shown.allowed),
            review: self.review && shown.allowed,
            cancel: self.cancel.clone(),
        })
    }

    /// The apply job started.
    pub fn applying(&mut self) {
        self.run = next_id();
        self.cancel = Arc::new(AtomicBool::new(false));
        self.phase = Phase::Applying {
            done: 0,
            total: self.ids.len(),
        };
    }

    /// A message for this dialog (answers for an older run or ticket are dropped).
    pub fn handle(&mut self, step: MatchStep) {
        match step {
            MatchStep::Picked {
                ticket,
                path: Some(path),
            } if ticket == self.ticket => {
                self.reference = Some(Reference::File(path));
                self.phase = Phase::Choose;
            }
            MatchStep::Progress { run, text } if run == self.run => {
                if matches!(self.phase, Phase::Running(_)) {
                    self.phase = Phase::Running(text);
                }
            }
            MatchStep::Fitted { run, result } if run == self.run => {
                self.phase = match result {
                    Ok(fitted) => Phase::Ready(fitted),
                    Err(e) => Phase::Failed(e.to_string()),
                };
            }
            MatchStep::Applying { run, done, total } if run == self.run => {
                self.phase = Phase::Applying { done, total };
            }
            MatchStep::Applied { run, report } if run == self.run => {
                self.phase = Phase::Done(report);
            }
            _ => {}
        }
    }

    /// The fit or apply job panicked.
    pub fn job_failed(&mut self, message: &str) {
        self.phase = Phase::Failed(format!("Something went wrong. {message}"));
    }

    /// Closing: a running fit or apply stops at its next step.
    fn close(&mut self) -> MatchAction {
        self.cancel.store(true, Ordering::SeqCst);
        MatchAction::Close
    }

    pub fn show(&mut self, ctx: &egui::Context, gate: AiGate) -> MatchAction {
        let mut action = MatchAction::None;
        let _ = egui::Modal::new(egui::Id::unique("match-reference")).show(ctx, |ui| {
            ui.set_max_width(520.0);
            ui.heading(TITLE);
            ui.add_space(8.0);
            action = match self.phase {
                Phase::Choose | Phase::Failed(_) => self.choose_ui(ui, gate),
                Phase::Running(_) => self.running_ui(ui),
                Phase::Ready(_) => self.ready_ui(ui),
                Phase::Applying { .. } | Phase::Done(_) => self.apply_ui(ui),
            };
        });
        if matches!(action, MatchAction::None) && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            action = self.close();
        }
        action
    }

    fn choose_ui(&mut self, ui: &mut egui::Ui, gate: AiGate) -> MatchAction {
        let mut action = MatchAction::None;
        ui.horizontal(|ui| {
            if ui.button(CHOOSE).clicked() {
                action = MatchAction::PickFile;
            }
            let chosen = match &self.reference {
                Some(Reference::Photo { id, .. }) => self
                    .ids
                    .iter()
                    .position(|i| i == id)
                    .map(|i| self.names[i].clone()),
                _ => None,
            };
            let mut pick = None;
            egui::ComboBox::from_label(OR_PHOTO)
                .selected_text(chosen.unwrap_or_default())
                .show_ui(ui, |ui| {
                    for (i, name) in self.names.iter().enumerate() {
                        if ui.selectable_label(false, name).clicked() {
                            pick = Some(i);
                        }
                    }
                });
            if let Some(i) = pick {
                self.reference = Some(Reference::Photo {
                    id: self.ids[i],
                    path: self.photos[i].path.clone(),
                });
            }
        });
        let Some(reference) = &self.reference else {
            return action;
        };
        ui.label(format!(
            "Reference: {}",
            crate::develop::open::file_name(reference.path())
        ));
        ui.add_space(6.0);
        ui.label(
            RichText::new(sample_text(self.sample_count(), self.ids.len())).color(theme::MUTED),
        );
        if gate.allowed {
            ui.label(RichText::new(SENT).color(theme::MUTED));
            let n = if self.review { 2 } else { 1 };
            ui.label(RichText::new(steps::cost_line(n, gate.remaining)).strong());
            ui.checkbox(&mut self.review, REVIEW);
        } else {
            let off = if gate.used_up {
                steps::USED_UP
            } else {
                steps::AI_OFF
            };
            ui.label(RichText::new(off).color(theme::MUTED));
        }
        if let Phase::Failed(message) = &self.phase {
            ui.label(RichText::new(message).color(theme::DANGER));
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui
                .button(if gate.allowed { READ } else { MEASURE })
                .clicked()
            {
                action = MatchAction::Run(gate);
            }
            if ui.button(CANCEL).clicked() {
                action = self.close();
            }
        });
        action
    }

    fn running_ui(&mut self, ui: &mut egui::Ui) -> MatchAction {
        let mut action = MatchAction::None;
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(self.status().unwrap_or_default().to_owned());
        });
        ui.add_space(8.0);
        if ui.button(CANCEL).clicked() {
            action = self.close();
        }
        action
    }

    fn ready_ui(&mut self, ui: &mut egui::Ui) -> MatchAction {
        let mut action = MatchAction::None;
        let Phase::Ready(fitted) = &mut self.phase else {
            return action;
        };
        let o = &fitted.outcome;
        if !o.notes.is_empty() {
            ui.label(&o.notes);
        }
        if let Some(note) = &o.ai_note {
            ui.label(RichText::new(note).color(theme::MUTED).small());
        }
        if fitted.all_edited {
            ui.label(RichText::new(ALL_EDITED).color(theme::MUTED).small());
        }
        if fitted.skipped > 0 {
            ui.label(
                RichText::new(skipped_text(fitted.skipped))
                    .color(theme::MUTED)
                    .small(),
            );
        }
        if o.requests_used > 0 {
            ui.label(
                RichText::new(format!("AI requests used: {}.", o.requests_used))
                    .color(theme::MUTED)
                    .small(),
            );
        }
        ui.add_space(6.0);
        let tiles = fitted.tiles;
        self.preview.update(
            ui.ctx(),
            fitted.renderer.as_mut(),
            &fitted.sheet,
            tiles,
            &fitted.outcome.settings,
            self.strength,
        );
        ui.horizontal(|ui| {
            if ui
                .add_enabled(self.preview.index > 0, egui::Button::new("‹"))
                .on_hover_text("Previous photo")
                .clicked()
            {
                self.preview.index -= 1;
            }
            let size = egui::vec2(160.0, 160.0);
            let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
            self.before = response.is_pointer_button_down_on();
            if let Some(t) = self.preview.texture(self.before) {
                egui::Image::new(egui::load::SizedTexture::new(t.id(), size)).paint_at(ui, rect);
            }
            let tag = if self.before { "Before" } else { "After" };
            ui.painter().text(
                rect.left_bottom() + egui::vec2(4.0, -4.0),
                egui::Align2::LEFT_BOTTOM,
                tag,
                egui::FontId::proportional(11.0),
                theme::INK,
            );
            if ui
                .add_enabled(self.preview.index + 1 < tiles, egui::Button::new("›"))
                .on_hover_text("Next photo")
                .clicked()
            {
                self.preview.index += 1;
            }
            ui.label(
                RichText::new(format!("{HOLD} {} of {tiles}.", self.preview.index + 1))
                    .color(theme::MUTED)
                    .small(),
            );
        });
        if let Some(e) = &self.preview.error {
            ui.label(RichText::new(e).color(theme::DANGER).small());
        }
        ui.add(
            egui::Slider::new(&mut self.strength, 0.0..=100.0)
                .step_by(5.0)
                .suffix(" %")
                .text(STRENGTH),
        );
        ui.checkbox(&mut self.match_exposure, EXPOSURE);
        ui.label(RichText::new(EXPOSURE_NOTE).color(theme::MUTED).small());
        ui.label(PRESET);
        ui.add(
            egui::TextEdit::singleline(&mut self.preset_name)
                .hint_text(PRESET_HINT)
                .char_limit(crate::dialogs::save_preset::MAX_NAME),
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button(batch::apply_label(self.ids.len())).clicked() {
                action = MatchAction::Apply(Box::new(self.plan()));
            }
            if ui.button(CANCEL).clicked() {
                action = self.close();
            }
        });
        action
    }

    /// What Apply writes, from the fitted grade and the choices.
    pub fn plan(&self) -> ApplyPlan {
        let Some(fitted) = self.fitted() else {
            return ApplyPlan {
                grade: EditSettings::default_settings(),
                exposure: None,
                preset: None,
            };
        };
        let name = crate::dialogs::save_preset::clean_name(&self.preset_name);
        ApplyPlan {
            grade: apply_strength(&fitted.outcome.settings, self.strength),
            exposure: self
                .match_exposure
                .then(|| (fitted.outcome.reference_median, fitted.medians.clone())),
            preset: (!name.is_empty()).then_some(name),
        }
    }

    fn apply_ui(&mut self, ui: &mut egui::Ui) -> MatchAction {
        let mut action = MatchAction::None;
        match &self.phase {
            Phase::Applying { done, total } => {
                ui.add(egui::ProgressBar::new(
                    *done as f32 / (*total).max(1) as f32,
                ));
                ui.label(RichText::new(batch::KEEP_OPEN).color(theme::MUTED).small());
                if ui.button(STOP).clicked() {
                    self.cancel.store(true, Ordering::SeqCst);
                }
            }
            Phase::Done(report) => {
                ui.label(format!(
                    "{} of {} edited.",
                    report.edited.len(),
                    report.total
                ));
                for (id, e) in &report.failed {
                    let name = self
                        .ids
                        .iter()
                        .position(|i| i == id)
                        .map_or_else(String::new, |i| self.names[i].clone());
                    ui.label(RichText::new(format!("{name}: {e}")).color(theme::DANGER));
                }
                if ui.button(DONE).clicked() {
                    action = MatchAction::Close;
                }
            }
            _ => {}
        }
        action
    }
}

/// Loads the reference: a catalogued photo opened as the samples are (at `AI_EDGE`); a file
/// through the same path, or as a plain picture when that cannot read it.
pub fn load_reference(reference: &Reference, open: OpenFn<'_>) -> Result<RgbaImage, String> {
    match reference {
        Reference::Photo { path, .. } => {
            open(path, AI_EDGE).map_err(|e| format!("The reference photo could not be read. {e}"))
        }
        Reference::File(path) => open(path, AI_EDGE).or_else(|first| {
            image::open(path)
                .map(|img| scaled(&img.to_rgba8(), AI_EDGE))
                .map_err(|e| {
                    log::info!("match: the reference could not be opened as a photo: {first}");
                    format!("The reference picture could not be read. {e}")
                })
        }),
    }
}

/// The file at `path` rendered with default settings at `edge` long edge: decoded at
/// `Quality::Fast` within `AI_EDGE` (through the frame cache for RAW), drawn by a temporary
/// renderer. The grade is fitted to (and measured on) the untouched look.
pub fn open_default(
    gpu: &Arc<lf_pipeline::Gpu>,
    frames: Option<&lf_raw::cache::FrameCache>,
    path: &Path,
    edge: u32,
) -> Result<RgbaImage, String> {
    let defaults = EditSettings::default_settings();
    let develop = lf_settings::DevelopSettings::from_settings(&defaults);
    let quality = lf_raw::demosaic::Quality::Fast;
    let opened = match frames {
        Some(cache) => crate::develop::open::open_cached(path, quality, &develop, AI_EDGE, cache),
        None => lf_raw::open::open(path, quality, &develop, AI_EDGE),
    }
    .map_err(|e| e.to_string())?;
    let mut renderer = lf_raw::render::renderer_for(gpu, &opened).map_err(|e| e.to_string())?;
    renderer.pixels(&defaults, edge).map_err(|e| e.to_string())
}

/// Forwards progress text for `run`.
pub fn progress(report: &dyn Fn(MatchStep), run: u64, text: &str) {
    report(MatchStep::Progress {
        run,
        text: text.to_owned(),
    });
}

/// Wraps a step for the channel.
pub fn msg(step: MatchStep) -> Msg {
    Msg::Match(step)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{Jobs, Lane};
    use lf_catalog::{NewPhoto, edits};
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;
    use std::time::{Duration, Instant};

    struct Flat;

    impl SheetRender for Flat {
        fn pixels(&mut self, _: &EditSettings, edge: u32) -> Result<RgbaImage, String> {
            Ok(RgbaImage::from_pixel(
                edge,
                edge / 2,
                image::Rgba([90, 90, 90, 255]),
            ))
        }
    }

    fn rows(n: i64) -> Vec<PhotoRow> {
        (1..=n).map(|id| samples::tests::row(id, false)).collect()
    }

    #[test]
    fn cancel_stops_the_lane_job() {
        let jobs = Jobs::new(egui::Context::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let opened = Arc::new(AtomicUsize::new(0));
        let req = FitRequest {
            run: 3,
            photos: rows(20),
            reference: Reference::File(PathBuf::from("ref.jpg")),
            ai: None,
            review: false,
            cancel: cancel.clone(),
        };
        let (flag, count) = (cancel.clone(), opened.clone());
        let reporter = jobs.reporter();
        jobs.spawn(Lane::Open, "match", move || {
            let open = |_: &Path, edge: u32| {
                // The dialog is closed while the third photo is being read.
                if count.fetch_add(1, Ordering::SeqCst) == 2 {
                    flag.store(true, Ordering::SeqCst);
                }
                Ok(RgbaImage::from_pixel(
                    edge,
                    edge,
                    image::Rgba([80, 80, 80, 255]),
                ))
            };
            let report = |s: MatchStep| reporter.send(msg(s));
            msg(fit(req, &open, |_| Ok(Box::new(Flat)), &report))
        });
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut texts = Vec::new();
        let result = loop {
            assert!(Instant::now() < deadline, "the job never ended");
            let mut done = None;
            for m in jobs.drain() {
                match m {
                    Msg::Match(MatchStep::Progress { run: 3, text }) => texts.push(text),
                    Msg::Match(MatchStep::Fitted { run: 3, result }) => done = Some(result),
                    other => panic!("unexpected {other:?}"),
                }
            }
            if let Some(r) = done {
                break r;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        assert!(matches!(result, Err(MatchError::Cancelled)), "{result:?}");
        assert_eq!(
            opened.load(Ordering::SeqCst),
            3,
            "no photo opened after the close"
        );
        assert_eq!(
            texts.first().map(String::as_str),
            Some("Reading your photos… 0 of 20")
        );
        assert!(!texts.iter().any(|t| t.starts_with("Fitting")));
    }

    #[test]
    fn a_fit_without_ai_reports_its_steps_and_the_medians() {
        let req = FitRequest {
            run: 1,
            photos: rows(3),
            reference: Reference::Photo {
                id: 2,
                path: PathBuf::from("2.jpg"),
            },
            ai: None,
            review: true,
            cancel: Arc::new(AtomicBool::new(false)),
        };
        let edges = Mutex::new(Vec::new());
        let open = |p: &Path, edge: u32| {
            edges.lock().unwrap().push((p.to_path_buf(), edge));
            Ok(RgbaImage::from_pixel(
                edge,
                edge,
                image::Rgba([64, 64, 64, 255]),
            ))
        };
        let texts = Mutex::new(Vec::new());
        let report = |s: MatchStep| {
            if let MatchStep::Progress { text, .. } = s {
                texts.lock().unwrap().push(text);
            }
        };
        let MatchStep::Fitted { run: 1, result } = fit(req, &open, |_| Ok(Box::new(Flat)), &report)
        else {
            panic!("not a fit answer");
        };
        let fitted = result.unwrap();
        assert_eq!(fitted.tiles, 3);
        assert_eq!(fitted.outcome.ai_note.as_deref(), Some(steps::AI_OFF));
        assert!((fitted.medians[&1] - 64.0 / 255.0).abs() < 0.01);
        let edges = edges.lock().unwrap();
        // The samples are read at 512 (as the web) and cover-fitted into 256 px tiles.
        assert_eq!(edges.iter().filter(|(_, e)| *e == MEASURE_EDGE).count(), 3);
        assert_eq!(fitted.sheet.width(), 3 * TILE_EDGE);
        assert!(
            edges.contains(&(PathBuf::from("2.jpg"), AI_EDGE)),
            "the reference at 1024"
        );
        let texts = texts.lock().unwrap();
        assert_eq!(texts[0], reading_text(0, 3));
        assert!(texts.contains(&steps::READING.to_owned()));
        assert!(texts.contains(&steps::fitting(1)));
    }

    fn catalogue(n: usize) -> (tempfile::TempDir, Catalog, Vec<i64>) {
        let dir = tempfile::tempdir().unwrap();
        let c = Catalog::open(dir.path()).unwrap();
        let folder = c.add_folder(&dir.path().to_string_lossy()).unwrap();
        let ids = (0..n)
            .map(|i| {
                let path = dir.path().join(format!("{i}.jpg"));
                std::fs::write(&path, b"x").unwrap();
                c.add_photo(
                    folder,
                    &NewPhoto {
                        path: path.to_string_lossy().into_owned(),
                        kind: "jpeg".into(),
                        ..Default::default()
                    },
                )
                .unwrap()
            })
            .collect();
        (dir, c, ids)
    }

    #[test]
    fn exposure_match_offsets_each_photo() {
        let (dir, c, ids) = catalogue(3);
        // The first photo has a crop and an edit already; both are kept apart from the look.
        let mut cropped = EditSettings::default_settings();
        cropped.crop.w = 0.5;
        cropped.exposure = -2.0;
        cropped.vibrance = 40.0;
        edits::set_settings(&c, ids[0], &cropped).unwrap();

        let mut grade = EditSettings::default_settings();
        grade.exposure = 0.2;
        grade.temperature = 10.0;
        // Reference median 0.5. Photo 0 was sampled at 0.25 (+1 EV), photo 1 at 0.5 (none);
        // photo 2 was not sampled, so it is measured now: 0.125 (+2, clamped to +1.5).
        let medians: HashMap<i64, f64> = [(ids[0], 0.25), (ids[1], 0.5)].into();
        let exposure = Some((0.5, medians));
        let measured = AtomicUsize::new(0);
        let open = |_: &Path, edge: u32| {
            measured.fetch_add(1, Ordering::SeqCst);
            assert_eq!(edge, TILE_EDGE);
            let v = (0.125f64 * 255.0).round() as u8;
            Ok(RgbaImage::from_pixel(8, 8, image::Rgba([v, v, v, 255])))
        };
        let mut offset = offsets(&exposure, &open);
        let mut seen = Vec::new();
        let report = apply(
            &c,
            &ids,
            &grade,
            &mut offset,
            &SidecarSaver::new(),
            &AtomicBool::new(false),
            &mut |done| seen.push(done),
        );
        assert_eq!(report.edited, ids);
        assert!(report.failed.is_empty());
        assert_eq!(seen, [1, 2, 3]);
        assert_eq!(
            measured.load(Ordering::SeqCst),
            1,
            "only the unsampled photo"
        );
        let got = |id| edits::get_settings(&c, id).unwrap().unwrap();
        let close = |a: f64, b: f64| (a - b).abs() < 0.02;
        assert!(close(got(ids[0]).exposure, 1.2), "{}", got(ids[0]).exposure);
        assert!(close(got(ids[1]).exposure, 0.2), "{}", got(ids[1]).exposure);
        assert!(close(got(ids[2]).exposure, 1.7), "{}", got(ids[2]).exposure);
        for id in &ids {
            assert_eq!(got(*id).temperature, 10.0, "the colour grade is identical");
        }
        assert_eq!(got(ids[0]).crop.w, 0.5, "the crop stays the photo's own");
        assert_eq!(got(ids[0]).vibrance, 0.0, "the look groups are replaced");
        let side = lf_catalog::sidecar::path_for(&dir.path().join("1.jpg"));
        assert!(side.exists(), "the sidecar is written");

        // Without exposure matching, every photo takes the grade's exposure.
        let mut none = offsets(&None, &open);
        apply(
            &c,
            &ids,
            &grade,
            &mut none,
            &SidecarSaver::new(),
            &AtomicBool::new(false),
            &mut |_| {},
        );
        for id in &ids {
            assert!(close(got(*id).exposure, 0.2));
        }
    }

    #[test]
    fn an_apply_stops_before_the_next_photo() {
        let (_d, c, ids) = catalogue(3);
        let stop = AtomicBool::new(false);
        let mut none = |_: &PhotoRow| Ok(None);
        let report = apply(
            &c,
            &ids,
            &EditSettings::default_settings(),
            &mut none,
            &SidecarSaver::new(),
            &stop,
            &mut |_| stop.store(true, Ordering::SeqCst),
        );
        assert_eq!((report.done, report.total), (1, 3));
        assert_eq!(report.edited, [ids[0]]);
    }

    /// The cost line is on screen before anything can be sent, follows the review toggle,
    /// and gives way to the measurements-only copy when AI is off.
    #[test]
    fn the_dialog_shows_the_cost_before_the_ai_is_asked() {
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable as _;
        struct S {
            dialog: MatchDialog,
            gate: AiGate,
            last: Option<MatchAction>,
        }
        let mut dialog = MatchDialog::new(rows(3));
        dialog.reference = Some(Reference::Photo {
            id: 2,
            path: PathBuf::from("2.jpg"),
        });
        let state = S {
            dialog,
            gate: AiGate {
                allowed: true,
                remaining: Some(5),
                used_up: false,
            },
            last: None,
        };
        let mut h = Harness::builder().with_size([700.0, 700.0]).build_ui_state(
            |ui, s: &mut S| {
                let action = s.dialog.show(ui.ctx(), s.gate);
                if !matches!(action, MatchAction::None) {
                    s.last = Some(action);
                }
            },
            state,
        );
        h.run();
        h.run();
        h.get_by_label(&steps::cost_line(2, Some(5)));
        h.get_by_label(REVIEW).click();
        h.run();
        h.get_by_label(&steps::cost_line(1, Some(5)));
        assert!(!h.state().dialog.review);
        h.get_by_label(READ).click();
        h.run();
        assert!(matches!(
            h.state().last,
            Some(MatchAction::Run(AiGate { allowed: true, .. }))
        ));

        h.state_mut().gate = AiGate::default();
        h.run();
        h.get_by_label(steps::AI_OFF);
        h.get_by_label(MEASURE).click();
        h.run();
        assert!(matches!(
            h.state().last,
            Some(MatchAction::Run(AiGate { allowed: false, .. }))
        ));
        h.state_mut().gate = AiGate {
            allowed: false,
            remaining: Some(0),
            used_up: true,
        };
        h.run();
        h.get_by_label(steps::USED_UP);
        assert!(h.query_by_label(&steps::cost_line(1, Some(5))).is_none());
    }

    /// The dialog said AI is off ("Measure the look"); by the time the fit starts the gate
    /// allows AI. Nothing is sent: the fit follows what was shown.
    #[test]
    fn a_fit_asked_without_ai_sends_nothing_when_the_gate_opens_meanwhile() {
        use httpmock::prelude::*;
        use steps::tests::{allowed, link};
        let server = MockServer::start();
        let grade = server.mock(|w, t| {
            w.method(POST).path("/api/desktop/grade");
            t.status(200)
                .json_body(serde_json::json!({"data": {"settings": {}}}));
        });
        let mut dialog = MatchDialog::new(rows(2));
        dialog.reference = Some(Reference::Photo {
            id: 1,
            path: PathBuf::from("1.jpg"),
        });
        assert!(dialog.review, "the review toggle is on by default");
        let (ai, checks) = link(&server, allowed(), allowed());
        let req = dialog.start(Some(ai), AiGate::default()).unwrap();
        assert!(req.ai.is_none(), "no AI link under a gate shown without AI");
        assert!(!req.review);
        let open = |_: &Path, edge: u32| {
            Ok(RgbaImage::from_pixel(
                edge,
                edge,
                image::Rgba([70, 70, 70, 255]),
            ))
        };
        let MatchStep::Fitted { result, .. } = fit(req, &open, |_| Ok(Box::new(Flat)), &|_| {})
        else {
            panic!("not a fit answer");
        };
        assert_eq!(
            result.unwrap().outcome.ai_note.as_deref(),
            Some(steps::AI_OFF)
        );
        grade.assert_calls(0);
        assert_eq!(checks.load(Ordering::SeqCst), 0);

        // Shown with AI and one request (no review): the review stays off whatever the toggle
        // says now.
        let shown = AiGate {
            allowed: true,
            remaining: Some(3),
            used_up: false,
        };
        dialog.review = false;
        let (ai, _) = link(&server, allowed(), allowed());
        let req = dialog.start(Some(ai), shown).unwrap();
        assert!(req.ai.is_some() && !req.review);
        dialog.review = true;
        let (ai, _) = link(&server, allowed(), allowed());
        let req = dialog.start(Some(ai), shown).unwrap();
        assert!(req.ai.is_some() && req.review);
    }

    #[test]
    fn the_gate_gives_the_cost_line_its_numbers() {
        use lf_account::entitlement::{Entitlement, Standing};
        assert_eq!(AiGate::from(None), AiGate::default());
        let mut e = Entitlement::example(Standing::Active, true);
        e.ai.remaining = Some(7);
        assert_eq!(
            AiGate::from(Some(&Decision::Allow(e.clone()))),
            AiGate {
                allowed: true,
                remaining: Some(7),
                used_up: false,
            }
        );
        // None left: AI is off, with the used-up copy (the web's `aiUsable`).
        e.ai.remaining = Some(0);
        let gate = AiGate::from(Some(&Decision::Allow(e.clone())));
        assert!(!gate.allowed && gate.used_up);
        e.ai.remaining = None;
        assert_eq!(AiGate::from(Some(&Decision::Allow(e))).remaining, None);
        let ro = Entitlement::example(Standing::ReadOnly, true);
        assert!(!AiGate::from(Some(&Decision::Allow(ro))).allowed);
        assert_eq!(
            sample_text(24, 30),
            "24 of your 30 selected photos will be read to work out one grade for all of them."
        );
    }
}
