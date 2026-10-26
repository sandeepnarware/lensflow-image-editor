//! The library: catalogue root, folders, the grid, the filmstrip and the selection.
//!
//! The UI thread holds no catalogue connection: quick reads (folders, a folder's photos) run
//! on the `Query` lane, short writes (removals, marking missing) on the `Io` lane and imports
//! and rescans on the `Long` lane, each job with its own `Catalog`, answering with a [`Msg`]. So a folder never waits behind an import to
//! show. The thumbnail pool has its own connections.

pub mod export_ui;
pub mod filmstrip;
pub mod grid;
pub mod import_ui;
pub mod selection;

use crate::app::THUMBS_STOPPED_BANNER;
use crate::catalog_ext::{self, count};
use crate::clipboard::{self, Clipboard};
use crate::develop::{self, Develop, Outcome};
use crate::dialogs::batch::{self, BatchAction, BatchDialog, BatchProgress, Choice};
use crate::dialogs::export::ExportDialog;
use crate::dialogs::group_picker::{CopyAction, CopyDialog};
use crate::dialogs::match_reference::steps::{AiLink, MatchError, SheetRender};
use crate::dialogs::match_reference::{
    self as matching, AiGate, MatchAction, MatchDialog, MatchStep,
};
use crate::jobs::{Jobs, Lane, Msg};
use crate::prefs::Prefs;
use crate::shortcuts::{Action, Keys, MAP};
use crate::theme;
use crate::thumbs_ui::{ThumbEvent, Thumbs};
use crate::toast::{Level, Toasts};
use eframe::egui::{self, Key, RichText};
use eframe::egui_wgpu::RenderState;
use lf_catalog::{Folder, PhotoRow};
use lf_pipeline::Gpu;
use lf_raw::cache::FrameCache;
use selection::{Dir, Selection};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How long a paste waits for the system clipboard's text.
const PASTE_WAIT: Duration = Duration::from_secs(1);

/// Shown on a photo whose sidecar could not be read (Develop explains it).
pub const SIDECAR_BADGE: &str = "Edit file damaged";
/// Shown on a photo whose thumbnail could not be made.
pub const FAILED_BADGE: &str = "Can't open";
/// Shown on a photo whose file is gone.
pub const MISSING_BADGE: &str = "Missing";

/// What the app needs from the library's surroundings while it handles a message or draws.
pub struct Env<'a> {
    pub jobs: &'a Jobs,
    pub toasts: &'a mut Toasts,
    pub prefs: &'a mut Prefs,
    pub config_dir: &'a Path,
}

impl Env<'_> {
    fn save_prefs(&self) {
        if let Err(e) = self.prefs.save(self.config_dir) {
            log::error!("could not save preferences: {e}");
        }
    }
}

/// Where the catalogue root stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage {
    /// No root chosen yet (first run).
    NoRoot,
    /// The chosen root is on a network drive: ask first.
    NetworkWarning(PathBuf),
    /// A root is set and not opened yet.
    Closed,
    Opening,
    /// The catalogue could not be opened (the message says why).
    Failed(String),
    Ready,
}

/// Which view the window shows for the signed-in user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Library,
    /// Develop with this photo.
    Develop(i64),
}

pub struct Library {
    gpu: Arc<Gpu>,
    frames_dir: PathBuf,
    root: Option<PathBuf>,
    pub stage: Stage,
    pub mode: Mode,
    folders: Vec<Folder>,
    current_folder: Option<i64>,
    photos: Vec<PhotoRow>,
    /// Photo ids in display order (the same order as `photos`).
    order: Vec<i64>,
    /// Index of each id in `order`.
    index: HashMap<i64, usize>,
    pub selection: Selection,
    thumbs: Thumbs,
    frames: Option<Arc<FrameCache>>,
    /// The Develop view (its renderer lives only while a photo is open in it).
    develop: Develop,
    /// The "Add folder" picker or a scan is running; the text says which.
    busy: Option<&'static str>,
    importing: Option<import_ui::Importing>,
    /// Set when the app quits: a running import stops after its batch.
    stop_import: Arc<AtomicBool>,
    /// A thumbnail pool is being rebuilt after a device error.
    reviving: bool,
    /// Asking before a folder is removed from the catalogue.
    confirm_remove: Option<Folder>,
    /// The folder chooser (picker and typed path) for the root and for "Add folder".
    chooser: import_ui::Chooser,
    /// The "Add folder" dialog is open.
    adding: bool,
    /// Asking whether an emptied folder's photos (folder id, count) are gone.
    confirm_missing: Option<(i64, usize)>,
    /// The folder whose photos are being loaded.
    loading: Option<i64>,
    /// Scroll the grid and filmstrip to the active photo on this frame.
    reveal_active: bool,
    /// The grid's columns last frame (for the arrow keys).
    columns: usize,
    last_scroll: f32,
    /// Copied edits (`Ctrl+Shift+C`).
    clipboard: Clipboard,
    /// The "Copy edits" dialog (its groups stay chosen) and whether it is open.
    copy: CopyDialog,
    copying: bool,
    /// A paste asked the system for its clipboard's text at this time.
    paste_asked: Option<Instant>,
    /// "Apply to N photos".
    batch: Option<BatchDialog>,
    /// "Match a reference…", the gate's last answer for its cost line, and a fit asked for
    /// (the app supplies the AI link).
    matching: Option<MatchDialog>,
    match_gate: AiGate,
    match_wanted: Option<AiGate>,
    /// "Export N photos", while open or while its export runs (hidden).
    export: Option<ExportDialog>,
    /// Preferences → Change…: the catalogue in use until another is chosen (the first-run
    /// screen offers to keep it).
    keep: Option<PathBuf>,
    /// The keyboard map's reader.
    keys: Keys,
}

impl Library {
    pub fn new(gpu: Arc<Gpu>, frames_dir: PathBuf, prefs: &Prefs) -> Library {
        let root = prefs.catalogue_root.clone();
        Library {
            gpu: gpu.clone(),
            frames_dir,
            stage: if root.is_some() {
                Stage::Closed
            } else {
                Stage::NoRoot
            },
            root,
            mode: Mode::Library,
            folders: Vec::new(),
            current_folder: None,
            photos: Vec::new(),
            order: Vec::new(),
            index: HashMap::new(),
            selection: Selection::default(),
            thumbs: Thumbs::default(),
            frames: None,
            develop: Develop::new(gpu),
            busy: None,
            importing: None,
            stop_import: Arc::new(AtomicBool::new(false)),
            reviving: false,
            confirm_remove: None,
            chooser: import_ui::Chooser::default(),
            adding: false,
            confirm_missing: None,
            loading: None,
            reveal_active: false,
            columns: 1,
            last_scroll: 0.0,
            clipboard: Clipboard::default(),
            copy: CopyDialog::default(),
            copying: false,
            paste_asked: None,
            batch: None,
            matching: None,
            match_gate: AiGate::default(),
            match_wanted: None,
            export: None,
            keep: None,
            keys: Keys::default(),
        }
    }

    /// Why the catalogue cannot be changed now (an export or "Apply to N photos" is
    /// running, "Match a reference" is open, or a folder is being read), if it cannot.
    pub fn restart_blocked(&self) -> Option<&'static str> {
        if self.export.as_ref().is_some_and(ExportDialog::running) {
            Some("Wait for the export to finish, then change the catalogue folder.")
        } else if self.batch.as_ref().is_some_and(BatchDialog::running) {
            Some("Wait for the edits to be applied, then change the catalogue folder.")
        } else if self.matching.is_some() {
            Some("Close “Match a reference” first, then change the catalogue folder.")
        } else if self.busy.is_some() {
            Some("Wait for the folder to be read, then change the catalogue folder.")
        } else {
            None
        }
    }

    /// Preferences → Change…, before the library is replaced: when nothing blocks it (else
    /// a toast says why and `None`), the open photo's gesture is committed, a running import
    /// is told to stop after its batch, and the waiting edit files are written, in that
    /// order. Returns the catalogue to offer back and the Develop generation to carry on.
    pub fn prepare_restart(&mut self, toasts: &mut Toasts) -> Option<(Option<PathBuf>, u64)> {
        if let Some(reason) = self.restart_blocked() {
            toasts.push(Level::Warning, reason);
            return None;
        }
        self.finish_edits(toasts);
        self.stop_import();
        if let Err(e) = self.sidecar_saver().flush() {
            toasts.push(
                Level::Error,
                format!("An edit file could not be written. {e}"),
            );
        }
        Some((self.root.clone(), self.develop.generation()))
    }

    /// A fresh library asking for another catalogue folder (Preferences → Change…): the
    /// first-run screen, with a way back to `keep`. `generation` is the old Develop view's,
    /// so none of its open answers still on the way is shown here.
    pub fn choose_another_root(&mut self, keep: Option<PathBuf>, generation: u64) {
        self.root = None;
        self.stage = Stage::NoRoot;
        self.keep = keep;
        self.develop.carry_generation(generation);
    }

    /// The catalogue folder offered back on the first-run screen after Change….
    pub fn kept_root(&self) -> Option<&Path> {
        self.keep.as_deref()
    }

    /// egui's renderer, for the Develop preview's shared texture (none in headless tests).
    pub fn set_render_state(&mut self, render_state: Option<RenderState>) {
        self.develop.set_render_state(render_state);
    }

    /// The Develop view.
    pub fn develop(&self) -> &Develop {
        &self.develop
    }

    pub fn develop_mut(&mut self) -> &mut Develop {
        &mut self.develop
    }

    /// The catalogue root in use.
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// The decoded-frame cache, once the catalogue is open (Develop uses it).
    pub fn frames(&self) -> Option<&FrameCache> {
        self.frames.as_deref()
    }

    /// The photo with this id in the current folder.
    pub fn photo(&self, id: i64) -> Option<&PhotoRow> {
        self.index.get(&id).map(|&i| &self.photos[i])
    }

    /// The folder shown, if any.
    pub fn current_folder(&self) -> Option<i64> {
        self.current_folder
    }

    /// The photos of the current folder, in display order.
    pub fn photos(&self) -> &[PhotoRow] {
        &self.photos
    }

    /// The thumbnails: made (as textures), failed, and still on their way.
    pub fn thumbnail_counts(&self) -> (usize, usize, usize) {
        (
            self.thumbs.texture_count(),
            self.thumbs.failed_count(),
            self.thumbs.pending(),
        )
    }

    /// A folder's photos are being loaded.
    pub fn loading(&self) -> bool {
        self.loading.is_some()
    }

    /// An import is running.
    pub fn importing(&self) -> bool {
        self.importing.is_some()
    }

    /// Stops a running import after its current batch, and a match at its next step (the app
    /// is quitting).
    pub fn stop_import(&self) {
        self.stop_import.store(true, Ordering::SeqCst);
        if let Some(m) = &self.matching {
            m.cancel.store(true, Ordering::SeqCst);
        }
    }

    /// What to ask before the window closes: an export or "Apply to N photos" is running.
    pub fn quit_question(&self) -> Option<&'static str> {
        if self.export.as_ref().is_some_and(ExportDialog::running) {
            Some(crate::dialogs::quit::EXPORT_RUNNING)
        } else if self.batch.as_ref().is_some_and(BatchDialog::running) {
            Some(crate::dialogs::quit::BATCH_RUNNING)
        } else {
            None
        }
    }

    /// The app is quitting: a running import stops after its batch, a match at its next
    /// step, and an export or "Apply to N photos" after the current photo.
    pub fn stop_long_work(&self) {
        self.stop_import();
        if let Some(export) = &self.export {
            export.stop.store(true, Ordering::SeqCst);
        }
        if let Some(batch) = &self.batch {
            batch.stop.store(true, Ordering::SeqCst);
        }
    }

    /// The folder a running export writes to (its unfinished files are removed on quit).
    pub fn running_export_folder(&self) -> Option<PathBuf> {
        self.export
            .as_ref()
            .filter(|d| d.running())
            .and_then(|d| d.form.folder.clone())
    }

    /// The flag [`Library::stop_import`] sets (for the device-reset dialog's thread).
    pub fn import_stop_flag(&self) -> Arc<AtomicBool> {
        self.stop_import.clone()
    }

    /// The sidecar writer (flushed when the app quits).
    pub fn sidecar_saver(&self) -> develop::debounce::SidecarSaver {
        self.develop.saver()
    }

    /// The studio signed in to, for its presets (the app tells the library each frame).
    pub fn set_studio(&mut self, studio: Option<i64>) {
        self.develop.set_studio(studio);
    }

    /// A preset was saved or deleted since the last call (the app asks for a sync).
    pub fn take_presets_changed(&mut self) -> bool {
        self.develop.take_presets_changed()
    }

    /// The copied edits.
    pub fn clipboard(&self) -> &Clipboard {
        &self.clipboard
    }

    /// The "Apply to N photos" dialog, while open.
    pub fn batch(&self) -> Option<&BatchDialog> {
        self.batch.as_ref()
    }

    /// The "Match a reference" dialog, while open.
    pub fn matching(&self) -> Option<&MatchDialog> {
        self.matching.as_ref()
    }

    /// The gate's last answer, for the match's cost line (the app tells the library each
    /// frame).
    pub fn set_match_gate(&mut self, decision: Option<&lf_account::entitlement::Decision>) {
        self.match_gate = AiGate::from(decision);
    }

    /// The dialog asked for a fit since the last call (the app then calls
    /// [`Library::start_match`] with the AI link).
    pub fn take_match_run(&mut self) -> Option<AiGate> {
        self.match_wanted.take()
    }

    /// Opens "Match a reference" for the selected photos.
    pub fn open_match(&mut self) {
        let photos: Vec<PhotoRow> = self
            .selected_in_order()
            .into_iter()
            .filter_map(|id| self.photo(id).cloned())
            .collect();
        if !photos.is_empty() {
            self.matching = Some(MatchDialog::new(photos));
        }
    }

    /// Closes "Match a reference" (cancelling a fit or apply still running).
    pub fn close_match(&mut self) {
        if let Some(m) = self.matching.take() {
            m.cancel.store(true, Ordering::SeqCst);
        }
    }

    /// Starts the fit on the `Long` lane (the device renders the sheet; opening photos for
    /// Develop never waits behind it); progress and the
    /// answer arrive as `Msg::Match`. `ai` is `None` without a studio session.
    pub fn start_match(&mut self, ai: Option<AiLink>, shown: AiGate, jobs: &Jobs) {
        let Some(req) = self.matching.as_mut().and_then(|m| m.start(ai, shown)) else {
            return;
        };
        let gpu = self.gpu.clone();
        let frames = self.frames.clone();
        let reporter = jobs.reporter();
        jobs.spawn(Lane::Long, "match", move || {
            let open =
                |p: &Path, edge: u32| matching::open_default(&gpu, frames.as_deref(), p, edge);
            let report = |step: MatchStep| reporter.send(Msg::Match(step));
            let renderer = |sheet: &image::RgbaImage| {
                lf_pipeline::Renderer::new(gpu.clone(), sheet, sheet.width(), sheet.height())
                    .map(|r| Box::new(r) as Box<dyn SheetRender>)
                    .map_err(|e| format!("The contact sheet could not be drawn. {e}"))
            };
            Msg::Match(matching::fit(req, &open, renderer, &report))
        });
    }

    fn match_step(&mut self, step: MatchStep, env: &mut Env) -> Option<Msg> {
        match step {
            MatchStep::SignedOut(g) => return Some(Msg::Match(MatchStep::SignedOut(g))),
            MatchStep::Fitted {
                run,
                result: Err(MatchError::Unauthorized(g)),
            } => {
                if let Some(m) = &mut self.matching {
                    m.handle(MatchStep::Fitted {
                        run,
                        result: Err(MatchError::Unauthorized(g)),
                    });
                }
                return Some(Msg::Match(MatchStep::SignedOut(g)));
            }
            MatchStep::Applied { run, report } => {
                // Photos written before a close still get their new thumbnails and settings.
                self.edited(&report.edited, env);
                if let Some(m) = &mut self.matching {
                    m.handle(MatchStep::Applied { run, report });
                }
            }
            other => {
                if let Some(m) = &mut self.matching {
                    m.handle(other);
                }
            }
        }
        None
    }

    fn match_dialog(&mut self, ctx: &egui::Context, env: &mut Env) {
        let Some(dialog) = &mut self.matching else {
            return;
        };
        match dialog.show(ctx, self.match_gate) {
            MatchAction::None => {}
            MatchAction::Close => self.matching = None,
            MatchAction::Run(shown) => self.match_wanted = Some(shown),
            MatchAction::PickFile => {
                dialog.ticket = matching::next_id();
                let ticket = dialog.ticket;
                let reporter = env.jobs.reporter();
                // Its own short-lived thread, as the folder picker: one that never answers
                // must not hold up a lane.
                let spawned =
                    std::thread::Builder::new()
                        .name("lf-picker".into())
                        .spawn(move || {
                            let path = rfd::FileDialog::new()
                                .set_title(matching::CHOOSE)
                                .add_filter("Pictures", matching::IMAGES)
                                .pick_file();
                            reporter.send(Msg::Match(MatchStep::Picked { ticket, path }));
                        });
                if let Err(e) = spawned {
                    log::warn!("could not open the file picker: {e}");
                }
            }
            MatchAction::Apply(plan) => self.apply_match(*plan, env),
        }
    }

    /// Saves the preset when named, then writes the grade to every selected photo on the
    /// `Long` lane (measuring a photo needs the device), each through the batch's per-photo
    /// path.
    fn apply_match(&mut self, plan: matching::ApplyPlan, env: &mut Env) {
        let Some(root) = self.root.clone() else {
            env.toasts.push(Level::Error, matching::NO_CATALOGUE);
            return;
        };
        // The open photo's gesture is recorded first, so the grade lands on top of it.
        self.develop.finish_edit(env.toasts);
        if let Some(name) = &plan.preset {
            let partial =
                crate::clipboard::picked(&plan.grade, &lf_settings::groups::DEFAULT_COPY_GROUPS);
            match self.develop.studio() {
                None => env
                    .toasts
                    .push(Level::Warning, crate::develop::panels::presets::NO_STUDIO),
                Some(studio) => match self.develop.save_preset(studio, name, &partial) {
                    Ok(true) => env
                        .toasts
                        .push(Level::Success, format!("Preset “{name}” saved.")),
                    Ok(false) => env
                        .toasts
                        .push(Level::Warning, crate::dialogs::save_preset::TAKEN),
                    Err(e) => env
                        .toasts
                        .push(Level::Error, format!("The preset could not be saved. {e}")),
                },
            }
        }
        let Some(dialog) = &mut self.matching else {
            return;
        };
        dialog.applying();
        let (run, ids, stop) = (dialog.run, dialog.ids.clone(), dialog.cancel.clone());
        let gpu = self.gpu.clone();
        let frames = self.frames.clone();
        let saver = self.develop.saver();
        let reporter = env.jobs.reporter();
        env.jobs.spawn(Lane::Long, "match apply", move || {
            let total = ids.len();
            let report = match lf_catalog::Catalog::open(&root) {
                Err(e) => matching::ApplyReport {
                    total,
                    done: total,
                    edited: Vec::new(),
                    failed: ids.iter().map(|&id| (id, e.to_string())).collect(),
                },
                Ok(catalog) => {
                    let open = |p: &Path, edge: u32| {
                        matching::open_default(&gpu, frames.as_deref(), p, edge)
                    };
                    let mut offset = matching::offsets(&plan.exposure, &open);
                    let mut progress = |done: usize| {
                        reporter.send(Msg::Match(MatchStep::Applying { run, done, total }))
                    };
                    matching::apply(
                        &catalog,
                        &ids,
                        &plan.grade,
                        &mut offset,
                        &saver,
                        &stop,
                        &mut progress,
                    )
                }
            };
            Msg::Match(MatchStep::Applied { run, report })
        });
    }

    /// Commits a gesture still in progress (the app is quitting).
    pub fn finish_edits(&mut self, toasts: &mut Toasts) {
        self.develop.finish_edit(toasts);
    }

    /// Handles the library's messages; anything else is handed back.
    pub fn handle(&mut self, msg: Msg, env: &mut Env) -> Option<Msg> {
        if self.stage != Stage::Ready
            && matches!(
                msg,
                Msg::Photos { .. }
                    | Msg::Scanned { .. }
                    | Msg::ImportProgress(..)
                    | Msg::ImportDone(..)
                    | Msg::MarkedMissing { .. }
                    | Msg::FolderRemoved { .. }
                    | Msg::PoolReady(..)
            )
        {
            // Work for a catalogue left behind (Preferences → Change…), answered before
            // this library's catalogue opened.
            log::debug!("library not open: dropped {msg:?}");
            return None;
        }
        if matches!(msg, Msg::ImportProgress(..) | Msg::ImportDone(..)) && self.importing.is_none()
        {
            // An import this library never started: one for a catalogue left behind
            // (Preferences → Change…, which waits for a scan but only tells an import to
            // stop) still finishing on the `Long` lane after this library's catalogue opened.
            log::debug!("dropped {msg:?}: not started here");
            return None;
        }
        match msg {
            Msg::FolderChosen {
                purpose,
                ticket,
                result,
            } => self.folder_chosen(purpose, ticket, result, env),
            Msg::CatalogueOpened { root, result } => self.opened(root, result, env),
            Msg::PoolReady(result) => {
                self.reviving = false;
                match result {
                    Ok(pool) => {
                        self.thumbs.set_pool(pool.0);
                        self.thumbs.background(&self.order);
                    }
                    Err(e) => log::error!("could not restart thumbnails: {e}"),
                }
            }
            Msg::MarkedMissing { folder_id, result } => self.marked_missing(folder_id, result, env),
            Msg::Scanned { folder, scan } => self.scanned(folder, scan, env),
            Msg::ImportProgress(progress) => {
                if let Some(importing) = &mut self.importing {
                    importing.progress = Some(progress);
                }
            }
            Msg::ImportDone(result) => self.import_done(result, env),
            Msg::Folders { root, result } => match result {
                // A list from a catalogue left behind (Preferences → Change…).
                _ if self.root.as_deref() != Some(root.as_path()) => {}
                Ok(folders) => self.folders_loaded(folders, env),
                Err(e) => env
                    .toasts
                    .push(Level::Error, format!("Could not list the folders. {e}")),
            },
            Msg::Photos { root, .. } if self.root.as_deref() != Some(root.as_path()) => {
                // A list from a catalogue left behind (Preferences → Change…): its folder ids
                // mean nothing here.
                log::debug!("dropped the photos of {}", root.display());
            }
            Msg::Photos {
                folder_id, result, ..
            } => {
                if self.loading == Some(folder_id) {
                    self.loading = None;
                }
                match result {
                    Ok(rows) => self.photos_loaded(folder_id, rows),
                    Err(e) => env
                        .toasts
                        .push(Level::Error, format!("Could not list the photos. {e}")),
                }
            }
            Msg::FolderRemoved { folder_id, result } => match result {
                Ok(()) => {
                    if self.current_folder == Some(folder_id) {
                        self.show_folder(None, env);
                    }
                    self.refresh_folders(env);
                }
                Err(e) => env.toasts.push(
                    Level::Error,
                    format!("Could not remove the folder from the catalogue. {e}"),
                ),
            },
            Msg::Opened {
                photo_id,
                generation,
                result,
            } => {
                let outcome = self
                    .develop
                    .handle_opened(photo_id, generation, result, env.toasts);
                self.opened_in_develop(outcome, env);
            }
            Msg::SidecarWritten(result) => self.develop.sidecar_written(result, env.toasts),
            Msg::Preopened { photo_id, result } => match result {
                Ok(true) => log::debug!("photo {photo_id} pre-opened into the frame cache"),
                Ok(false) => log::debug!("pre-opening photo {photo_id} skipped"),
                Err(e) => log::info!("could not pre-open photo {photo_id}: {e}"),
            },
            Msg::BatchProgress(progress) => self.batch_progress(progress, env),
            Msg::Match(step) => return self.match_step(step, env),
            Msg::ExportProgress { done, total } => self.export_progress(done, total),
            Msg::ExportDone(result) => self.export_done(result, env),
            Msg::PresetsSynced(..) => {
                // The catalogue's presets may have changed; the session handles the rest.
                self.develop.presets_stale();
                return Some(msg);
            }
            Msg::JobFailed { name, message } => {
                // A panicking library job must not leave the library waiting for it.
                match name.as_str() {
                    "open photo" => self.develop.open_failed(&message),
                    "add folder" | "rescan" => self.busy = None,
                    "photos" => self.loading = None,
                    "import" => self.importing = None,
                    "open catalogue" => self.stage = Stage::Failed(message.clone()),
                    "thumbnail pool" => self.reviving = false,
                    "match" | "match apply" => {
                        if let Some(m) = &mut self.matching {
                            m.job_failed(&message);
                        }
                    }
                    "export" => {
                        if self.export.as_ref().is_some_and(ExportDialog::running) {
                            self.export_done(Err(message.clone()), env);
                        }
                    }
                    "batch" => {
                        if let Some(b) = &mut self.batch {
                            let total = b.ids.len();
                            let done = b.progress().map_or(0, |p| p.done);
                            b.update(BatchProgress {
                                done,
                                total,
                                failed: Vec::new(),
                                finished: true,
                            });
                        }
                    }
                    _ => {}
                }
                return Some(Msg::JobFailed { name, message });
            }
            other => return Some(other),
        }
        None
    }

    fn opened(
        &mut self,
        root: PathBuf,
        result: Result<Box<catalog_ext::Opened>, String>,
        env: &mut Env,
    ) {
        if self.root.as_deref() != Some(root.as_path()) {
            return;
        }
        match result {
            Ok(opened) => {
                let opened = *opened;
                log::info!(
                    "catalogue open at {} ({} folders)",
                    root.display(),
                    opened.folders.len()
                );
                self.thumbs = Thumbs::default();
                self.thumbs.set_pool(opened.thumbs);
                self.frames = Some(Arc::new(opened.frames));
                self.develop.set_root(&root);
                self.stage = Stage::Ready;
                self.folders_loaded(opened.folders, env);
            }
            Err(e) => {
                log::error!("could not open the catalogue at {}: {e}", root.display());
                self.stage = Stage::Failed(e);
            }
        }
    }

    fn folders_loaded(&mut self, folders: Vec<Folder>, env: &mut Env) {
        self.folders = folders;
        let still_there = self
            .current_folder
            .is_some_and(|id| self.folders.iter().any(|f| f.id == id));
        if !still_there {
            let first = self.folders.first().map(|f| f.id);
            self.show_folder(first, env);
        }
    }

    fn photos_loaded(&mut self, folder_id: i64, rows: Vec<PhotoRow>) {
        if self.current_folder != Some(folder_id) {
            return;
        }
        // A photo whose file or row changed (a rescan) gets a new thumbnail.
        let before: HashMap<i64, &str> = self
            .photos
            .iter()
            .map(|p| (p.id, p.updated_at.as_str()))
            .collect();
        let changed: Vec<i64> = rows
            .iter()
            .filter(|r| before.get(&r.id).is_some_and(|u| *u != r.updated_at))
            .map(|r| r.id)
            .collect();
        for id in changed {
            self.thumbs.forget(id);
        }
        self.order = rows.iter().map(|r| r.id).collect();
        self.index = self
            .order
            .iter()
            .enumerate()
            .map(|(i, id)| (*id, i))
            .collect();
        self.photos = rows;
        self.selection.retain(&self.order);
        if self.selection.active().is_none()
            && let Some(&first) = self.order.first()
        {
            self.selection.click(first);
        }
        self.thumbs.background(&self.order);
    }

    /// Shows `folder` (or nothing) and loads its photos.
    fn show_folder(&mut self, folder: Option<i64>, env: &mut Env) {
        if self.current_folder != folder {
            self.selection = Selection::default();
            self.photos.clear();
            self.order.clear();
            self.index.clear();
            self.thumbs.reset();
            self.reveal_active = true;
        }
        self.current_folder = folder;
        self.loading = folder;
        self.revive_thumbs(env);
        if let (Some(folder_id), Some(root)) = (folder, self.root.clone()) {
            env.jobs.spawn(Lane::Query, "photos", move || {
                let result = catalog_ext::photos_in(&root, folder_id);
                Msg::Photos {
                    root,
                    folder_id,
                    result,
                }
            });
        }
    }

    fn refresh_folders(&self, env: &mut Env) {
        let Some(root) = self.root.clone() else {
            return;
        };
        env.jobs.spawn(Lane::Query, "folders", move || {
            let result = catalog_ext::folders(&root);
            Msg::Folders { root, result }
        });
    }

    /// After a device error, the next library action builds a fresh thumbnail pool (once
    /// the device works again).
    fn revive_thumbs(&mut self, env: &mut Env) {
        if !self.thumbs.stopped() || self.reviving || self.gpu.is_lost() {
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        self.reviving = true;
        let gpu = self.gpu.clone();
        env.jobs.spawn(Lane::Io, "thumbnail pool", move || {
            Msg::PoolReady(catalog_ext::pool(gpu, &root).map(catalog_ext::Pool))
        });
    }

    fn open_photo(&mut self, id: i64, env: &mut Env) {
        let Some(photo) = self.photo(id) else {
            return;
        };
        if photo.missing {
            env.toasts.push(
                Level::Warning,
                "This photo is missing from its folder, so it cannot be opened. Reconnect the \
                 drive or rescan the folder.",
            );
            return;
        }
        self.mode = Mode::Develop(id);
        self.request_develop(id, env);
    }

    /// Asks the Develop view to open photo `id` (the photo is in this folder and present).
    fn request_develop(&mut self, id: i64, env: &mut Env) {
        let (Some(photo), Some(root)) = (self.photo(id).cloned(), self.root.clone()) else {
            return;
        };
        let quality = develop::open::quality(env.prefs.develop_quality);
        self.develop.request(
            &photo,
            &root,
            self.frames.clone(),
            quality,
            env.jobs,
            env.toasts,
        );
    }

    /// After an open answer: pre-open the neighbours of a shown photo; badge one that
    /// cannot be opened.
    fn opened_in_develop(&mut self, outcome: Outcome, env: &mut Env) {
        match outcome {
            Outcome::Shown(id) => {
                let Some(&at) = self.index.get(&id) else {
                    return;
                };
                let neighbours: Vec<&PhotoRow> = [at.checked_sub(1), Some(at + 1)]
                    .into_iter()
                    .flatten()
                    .filter_map(|i| self.photos.get(i))
                    .collect();
                if let Some(root) = self.root.as_deref() {
                    let quality = develop::open::quality(env.prefs.develop_quality);
                    self.develop
                        .preopen(&neighbours, root, self.frames.clone(), quality, env.jobs);
                }
            }
            Outcome::Failed(id, message) => {
                self.thumbs.absorb_failures(vec![(id, message)]);
            }
            Outcome::Dropped => {}
        }
    }

    /// Keeps the Develop view in step with the mode: closed in the library, following the
    /// active photo in Develop.
    fn sync_develop(&mut self, env: &mut Env) {
        match self.mode {
            Mode::Library => {
                if self.develop.photo_id().is_some() {
                    self.develop.close(env.toasts);
                }
            }
            Mode::Develop(id) => {
                if self.photo(id).is_none() {
                    // The photo left the folder (a folder change, a rescan).
                    self.mode = Mode::Library;
                    self.develop.close(env.toasts);
                } else if (self.develop.photo_id() != Some(id) || self.develop.wants_reopen())
                    && !self.develop.is_lost()
                {
                    // A new photo, or the shown RAW's develop settings changed.
                    self.request_develop(id, env);
                }
            }
        }
    }

    /// Draws the library (or the root screens) inside the signed-in chrome.
    pub fn ui(&mut self, ui: &mut egui::Ui, env: &mut Env) {
        if self.stage == Stage::Closed {
            self.open_catalogue(env);
        }
        if self.stage != Stage::Ready {
            egui::CentralPanel::default_margins().show(ui, |ui| self.root_screen(ui, env));
            return;
        }
        for event in self.thumbs.pump(ui.ctx()) {
            match event {
                ThumbEvent::WorkerFailed(message) => env.toasts.push(
                    Level::Error,
                    format!("Thumbnails could not start. {message}"),
                ),
                ThumbEvent::DeviceError(message) => {
                    log::error!("thumbnails stopped on a graphics device error: {message}");
                }
            }
        }
        self.sync_develop(env);
        self.develop.tick(ui.ctx(), env.jobs, env.toasts);
        let edited = self.develop.take_edited(Instant::now());
        if !edited.is_empty() {
            self.refresh_photos(&edited, env);
        }

        if self.thumbs.stopped() && !self.gpu.is_lost() {
            egui::Panel::top("library-device").show(ui, |ui| {
                ui.add_space(4.0);
                ui.label(RichText::new(THUMBS_STOPPED_BANNER).color(theme::WARNING));
                ui.add_space(4.0);
            });
        }
        egui::Panel::bottom("filmstrip")
            .exact_size(filmstrip::HEIGHT)
            .show(ui, |ui| self.filmstrip(ui, env));
        egui::Panel::left("folders")
            .default_size(220.0)
            .resizable(true)
            .show(ui, |ui| self.sidebar(ui, env));
        if let Mode::Develop(id) = self.mode {
            let photo = self.index.get(&id).map(|&i| &self.photos[i]);
            let develop = &mut self.develop;
            egui::Panel::right("develop-side")
                .exact_size(histogram_panel_width())
                .show(ui, |ui| develop.side_ui(ui, photo, env.toasts));
        }
        match self.mode {
            Mode::Library => {
                egui::CentralPanel::default_margins().show(ui, |ui| self.grid(ui, env));
            }
            Mode::Develop(_) => {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(theme::BG))
                    .show(ui, |ui| self.develop.preview_ui(ui, env.toasts));
            }
        }
        self.confirm_remove_dialog(ui.ctx(), env);
        self.add_folder_dialog(ui.ctx(), env);
        self.confirm_missing_dialog(ui.ctx(), env);
        self.copy_dialog(ui.ctx(), env);
        self.waiting_paste(ui.ctx(), env);
        self.batch_dialog(ui.ctx(), env);
        self.match_dialog(ui.ctx(), env);
        self.export_dialog(ui.ctx(), env);
        self.reveal_active = false;
    }

    /// A dialog of the library's is open: the window's keys wait until it closes.
    fn dialog_open(&self) -> bool {
        self.confirm_remove.is_some()
            || self.confirm_missing.is_some()
            || self.adding
            || self.copying
            || self.batch.is_some()
            || self.matching.is_some()
            || self.export.as_ref().is_some_and(|d| d.visible)
    }

    /// Whether `action` (a menu item or a key) is possible now in the library or Develop.
    pub fn enabled(&self, action: Action) -> bool {
        if self.stage != Stage::Ready {
            return false;
        }
        let develop = matches!(self.mode, Mode::Develop(_));
        let selected = !self.selection.current.is_empty();
        match action {
            Action::AddFolder => self.can_add(),
            Action::Export => selected || self.export.is_some(),
            Action::Undo => develop && self.develop.can_undo(),
            Action::Redo => develop && self.develop.can_redo(),
            Action::CopySettings => self.copy_source().is_some(),
            Action::PasteSettings | Action::ApplyToSelected | Action::MatchReference => selected,
            Action::SelectAll | Action::Previous | Action::Next => !self.order.is_empty(),
            Action::Library => develop,
            Action::Develop => !develop && self.selection.active().is_some(),
            Action::Up | Action::Down => !develop && !self.order.is_empty(),
            Action::BeforeAfter
            | Action::Fit
            | Action::ActualSize
            | Action::ToggleZoom
            | Action::AddVersion
            | Action::ResetAll => develop && self.develop.is_ready(),
            Action::CropTool
            | Action::SpotTool
            | Action::LeaveTool
            | Action::ApplyCrop
            | Action::DeleteSpot => develop && self.develop.tool_enabled(action),
            Action::Quit
            | Action::Preferences
            | Action::SwitchStudio
            | Action::CheckEntitlement
            | Action::SignOut
            | Action::About
            | Action::ShowLogFolder
            | Action::KeyboardShortcuts => false,
        }
    }

    /// Does `action` when it is the library's and possible now; false otherwise (the app's
    /// own actions: Quit, Preferences, the Account and Help menus).
    pub fn run(&mut self, action: Action, ctx: &egui::Context, env: &mut Env) -> bool {
        if !self.enabled(action) {
            return false;
        }
        match action {
            Action::AddFolder => self.add_folder(env),
            Action::Export => self.open_export(env),
            Action::Undo => self.develop.undo(env.toasts),
            Action::Redo => self.develop.redo(env.toasts),
            Action::CopySettings => self.copying = true,
            Action::PasteSettings => self.paste(ctx, env),
            Action::ApplyToSelected => self.open_batch(None),
            // The filmstrip in Develop too: the open photo stays the active one.
            Action::SelectAll => self.selection.select_all(&self.order),
            Action::MatchReference => self.open_match(),
            Action::Library => {
                self.mode = Mode::Library;
                self.reveal_active = true;
            }
            Action::Develop => {
                if let Some(id) = self.selection.active() {
                    self.open_photo(id, env);
                }
            }
            Action::Previous => self.step(Dir::Left, env),
            Action::Next => self.step(Dir::Right, env),
            Action::Up => self.step(Dir::Up, env),
            Action::Down => self.step(Dir::Down, env),
            Action::BeforeAfter => self.develop.toggle_before(),
            Action::Fit | Action::ActualSize | Action::ToggleZoom => self.develop.zoom(action),
            Action::AddVersion => self.develop.ask_add_version(),
            Action::ResetAll => self.develop.reset_all(env.toasts),
            Action::CropTool
            | Action::SpotTool
            | Action::LeaveTool
            | Action::ApplyCrop
            | Action::DeleteSpot => self.develop.tool_command(action, env.toasts),
            _ => return false,
        }
        true
    }

    /// The keyboard map, once per frame before drawing: does the library's keys and returns
    /// the presses meant for the app (`others` says which of its actions are possible).
    /// Nothing fires while a widget has the keyboard or a dialog is open; the Before/After
    /// key shows the photo as it was while held.
    pub fn shortcuts(
        &mut self,
        ctx: &egui::Context,
        env: &mut Env,
        others: impl Fn(Action) -> bool,
    ) -> Vec<Action> {
        let blocked = self.dialog_open();
        let held = !blocked
            && crate::shortcuts::keys_free(ctx)
            && matches!(self.mode, Mode::Develop(_))
            && ctx.input(|i| MAP.is_held(i, Action::BeforeAfter));
        self.develop.set_before_held(held);
        let mut keys = self.keys;
        let fired = keys.read(ctx, blocked, |a| self.enabled(a) || others(a));
        self.keys = keys;
        let mut rest = Vec::new();
        for action in fired {
            if !self.run(action, ctx, env) {
                rest.push(action);
            }
        }
        rest
    }

    /// An arrow key: moves the selection in the grid; in Develop, opens the previous or next
    /// photo it can (stepping over missing ones).
    fn step(&mut self, dir: Dir, env: &mut Env) {
        if let Mode::Develop(current) = self.mode {
            let step = match dir {
                Dir::Left => -1,
                Dir::Right => 1,
                Dir::Up | Dir::Down => return,
            };
            let missing = |id: i64| self.photo(id).is_none_or(|p| p.missing);
            if let Some(id) = next_openable(&self.order, current, step, missing) {
                self.selection.click(id);
                self.reveal_active = true;
                self.open_photo(id, env);
            }
        } else {
            self.selection.arrow(dir, self.columns, &self.order);
            self.reveal_active = true;
        }
    }

    /// The library's half of the status bar: photos and selection, then whatever runs.
    /// Returns whether it drew anything (nothing until the catalogue is open).
    pub fn status_ui(&self, ui: &mut egui::Ui) -> bool {
        if self.stage != Stage::Ready {
            return false;
        }
        ui.label(status_text(self.photos.len(), self.selection.current.len()));
        if let Some(text) = self.busy {
            ui.separator();
            ui.label(RichText::new(text).color(theme::MUTED));
        }
        if let Some((done, total)) = self.export_status() {
            ui.separator();
            ui.label(crate::dialogs::export::status_text(done, total));
            ui.add(egui::ProgressBar::new(done as f32 / total.max(1) as f32).desired_width(160.0));
        }
        if let Some(importing) = &self.importing {
            ui.separator();
            ui.label(importing.text());
            if let Some(p) = importing.progress.filter(|p| p.total > 0) {
                ui.add(egui::ProgressBar::new(p.done as f32 / p.total as f32).desired_width(160.0));
            }
        }
        true
    }

    fn sidebar(&mut self, ui: &mut egui::Ui, env: &mut Env) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Folders").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(self.can_add(), egui::Button::new("Add folder…"))
                    .clicked()
                {
                    self.add_folder(env);
                }
            });
        });
        ui.separator();
        let mut chosen = None;
        let mut rescan = None;
        let can_add = self.can_add();
        // A folder being imported (on the `Long` lane) stays until its import ends.
        let importing = self.importing.as_ref().map(|i| i.folder.clone());
        egui::ScrollArea::vertical()
            .id_salt("folder-list")
            .show(ui, |ui| {
                for folder in &self.folders {
                    let selected = self.current_folder == Some(folder.id);
                    let label = format!("{}  ({})", folder.name(), count(folder.count as usize));
                    let response = ui
                        .selectable_label(selected, label)
                        .on_hover_text(folder.path.display().to_string());
                    if response.clicked() {
                        chosen = Some(folder.id);
                    }
                    response.context_menu(|ui| {
                        if ui
                            .add_enabled(can_add, egui::Button::new("Rescan"))
                            .clicked()
                        {
                            rescan = Some(folder.clone());
                            ui.close();
                        }
                        let removable = importing.as_deref() != Some(folder.path.as_path());
                        if ui
                            .add_enabled(removable, egui::Button::new("Remove from catalogue…"))
                            .clicked()
                        {
                            self.confirm_remove = Some(folder.clone());
                            ui.close();
                        }
                    });
                }
            });
        if let Some(id) = chosen {
            self.mode = Mode::Library;
            self.show_folder(Some(id), env);
        }
        if let Some(folder) = rescan {
            self.rescan(&folder, env);
        }
    }

    fn confirm_remove_dialog(&mut self, ctx: &egui::Context, env: &mut Env) {
        let Some(folder) = self.confirm_remove.clone() else {
            return;
        };
        let mut close = false;
        let mut remove = false;
        egui::Modal::new(egui::Id::unique("confirm-remove-folder")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.heading(format!("Remove “{}” from the catalogue?", folder.name()));
            ui.add_space(8.0);
            ui.label(remove_folder_text(folder.count as usize));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let button = egui::Button::new(
                    RichText::new("Remove from catalogue").color(theme::BRAND_CONTRAST),
                )
                .fill(theme::DANGER);
                if ui.add(button).clicked() {
                    remove = true;
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                    close = true;
                }
            });
        });
        if remove {
            if let Some(root) = self.root.clone() {
                let folder_id = folder.id;
                env.jobs
                    .spawn(Lane::Io, "remove folder", move || Msg::FolderRemoved {
                        folder_id,
                        result: catalog_ext::remove_folder(&root, folder_id),
                    });
            }
            close = true;
        }
        if close {
            self.confirm_remove = None;
        }
    }

    fn grid(&mut self, ui: &mut egui::Ui, env: &mut Env) {
        if self.loading.is_some() && self.photos.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() / 3.0);
                ui.spinner();
                ui.label(RichText::new("Loading photos…").color(theme::MUTED));
            });
            return;
        }
        if self.folders.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() / 3.0);
                ui.heading("Add a folder of photos to begin.");
                ui.label(
                    RichText::new("Photos stay where they are; the catalogue only remembers them.")
                        .color(theme::MUTED),
                );
                ui.add_space(12.0);
                if ui
                    .add_enabled(self.can_add(), egui::Button::new("Add folder…"))
                    .clicked()
                {
                    self.add_folder(env);
                }
            });
            return;
        }
        if self.photos.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() / 3.0);
                ui.label(RichText::new("No photos in this folder.").color(theme::MUTED));
            });
            return;
        }
        let actions = grid::show(ui, self);
        if (actions.scroll_y - self.last_scroll).abs() > 0.5 {
            self.last_scroll = actions.scroll_y;
            self.revive_thumbs(env);
        }
        if let Some(id) = actions.open {
            self.open_photo(id, env);
        }
    }

    fn filmstrip(&mut self, ui: &mut egui::Ui, env: &mut Env) {
        if let Some(id) = filmstrip::show(ui, self) {
            self.selection.click(id);
            // Develop follows the click, through the same check as any open (a missing
            // photo is never opened).
            if let Mode::Develop(_) = self.mode {
                self.open_photo(id, env);
            }
        }
    }

    /// The photo whose edits a copy takes: the open one in Develop, the active one in the grid.
    fn copy_source(&self) -> Option<i64> {
        match self.mode {
            Mode::Develop(id) => Some(id),
            Mode::Library => self.selection.active(),
        }
    }

    fn copy_dialog(&mut self, ctx: &egui::Context, env: &mut Env) {
        if !self.copying {
            return;
        }
        match self.copy.show(ctx) {
            CopyAction::None => {}
            CopyAction::Cancel => self.copying = false,
            CopyAction::Copy(groups) => {
                self.copying = false;
                let settings = self
                    .copy_source()
                    .and_then(|id| self.develop.photo_settings(id));
                if let Some(settings) = settings {
                    let text = self.clipboard.copy(&settings, &groups);
                    ctx.copy_text(text);
                    env.toasts.push(Level::Success, clipboard::COPIED);
                }
            }
        }
    }

    /// `Ctrl+Shift+V`: the copied edits, or else the system clipboard's text (asked for
    /// here, answered by egui in a later frame).
    fn paste(&mut self, ctx: &egui::Context, env: &mut Env) {
        if self.selection.current.is_empty() {
            return;
        }
        match self.clipboard.slot().cloned() {
            Some(partial) => self.paste_settings(partial, env),
            None => {
                ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                self.paste_asked = Some(Instant::now());
            }
        }
    }

    /// Waits for the system clipboard's text after a paste found nothing copied here.
    fn waiting_paste(&mut self, ctx: &egui::Context, env: &mut Env) {
        let Some(asked) = self.paste_asked else {
            return;
        };
        let text = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Paste(text) => Some(text.clone()),
                _ => None,
            })
        });
        match text {
            Some(text) if text.len() > clipboard::MAX_TEXT => {
                self.paste_asked = None;
                env.toasts.push(Level::Info, clipboard::TOO_LONG);
            }
            Some(text) => {
                self.paste_asked = None;
                match self.clipboard.source(Some(&text)) {
                    Some(partial) => self.paste_settings(partial, env),
                    None => env.toasts.push(Level::Info, clipboard::NOTHING),
                }
            }
            None if asked.elapsed() > PASTE_WAIT => {
                self.paste_asked = None;
                env.toasts.push(Level::Info, clipboard::NOTHING);
            }
            None => ctx.request_repaint_after(Duration::from_millis(50)),
        }
    }

    /// The selected photos, in display order.
    fn selected_in_order(&self) -> Vec<i64> {
        self.order
            .iter()
            .copied()
            .filter(|id| self.selection.current.contains(id))
            .collect()
    }

    /// Pastes `partial` onto every selected photo: the open photo in Develop through its edit
    /// (one history step), one other photo at once, several through "Apply to N photos".
    pub fn paste_settings(&mut self, partial: serde_json::Value, env: &mut Env) {
        let targets = self.selected_in_order();
        match (self.mode, targets.as_slice()) {
            (_, []) => {}
            (Mode::Develop(open), [only]) if *only == open => {
                if let Some(edit) = self.develop.edit_state() {
                    let next = clipboard::apply(&edit.settings, &partial);
                    self.develop.replace_settings(next, env.toasts);
                }
            }
            (_, [only]) => {
                let id = *only;
                let recipe = clipboard::Recipe::Paste(partial);
                if self.develop.apply_to_photo(id, &recipe, env.toasts) {
                    self.edited(&[id], env);
                }
            }
            _ => self.open_batch(Some(partial)),
        }
    }

    /// Opens "Apply to N photos" for the selection; `pasted` comes first when given.
    fn open_batch(&mut self, pasted: Option<serde_json::Value>) {
        let ids = self.selected_in_order();
        if ids.is_empty() {
            return;
        }
        let names = ids
            .iter()
            .map(|id| {
                self.photo(*id)
                    .map(|p| develop::open::file_name(&p.path))
                    .unwrap_or_default()
            })
            .collect();
        let mut choices = Vec::new();
        if let Some(partial) = pasted.or_else(|| self.clipboard.slot().cloned()) {
            choices.push(Choice::pasted(partial));
        }
        if let Some(settings) = self
            .copy_source()
            .and_then(|id| self.develop.photo_settings(id))
        {
            choices.push(batch::current_choice(&settings));
        }
        for row in self.develop.preset_rows() {
            choices.push(Choice::preset(&row.name, row.settings));
        }
        self.batch = Some(BatchDialog::new(ids, names, choices));
    }

    fn batch_dialog(&mut self, ctx: &egui::Context, env: &mut Env) {
        let Some(dialog) = &mut self.batch else {
            return;
        };
        match dialog.show(ctx) {
            BatchAction::None => {}
            BatchAction::Closed => self.batch = None,
            BatchAction::Start(recipe) => {
                let Some(root) = self.root.clone() else {
                    return;
                };
                // The open photo's gesture is recorded first, so the batch builds on it.
                self.develop.finish_edit(env.toasts);
                let Some(dialog) = &mut self.batch else {
                    return;
                };
                dialog.started();
                batch::spawn(
                    env.jobs,
                    root,
                    dialog.ids.clone(),
                    recipe,
                    self.develop.saver(),
                    dialog.stop.clone(),
                );
            }
        }
    }

    fn batch_progress(&mut self, progress: BatchProgress, env: &mut Env) {
        let Some(dialog) = &mut self.batch else {
            return;
        };
        let finished = progress.finished;
        let done: Vec<i64> = dialog
            .ids
            .iter()
            .take(progress.done)
            .copied()
            .filter(|id| !progress.failed.iter().any(|(f, _)| f == id))
            .collect();
        if dialog.update(progress) {
            self.batch = None;
        }
        if finished {
            self.edited(&done, env);
        }
    }

    /// Photos edited outside the open photo's edit (a paste, a batch): new thumbnails, the
    /// edited badges, and the open photo takes its new settings.
    fn edited(&mut self, ids: &[i64], env: &mut Env) {
        if let Mode::Develop(open) = self.mode
            && ids.contains(&open)
        {
            self.develop.reload_edit(env.toasts);
        }
        self.refresh_photos(ids, env);
    }

    /// Photos whose edits changed in the catalogue: their thumbnails are made again (asked
    /// for at `High`) and the folder's rows are read again (the edited badge).
    fn refresh_photos(&mut self, ids: &[i64], env: &mut Env) {
        for id in ids {
            self.thumbs.forget(*id);
        }
        let shown: Vec<i64> = ids
            .iter()
            .copied()
            .filter(|id| self.index.contains_key(id))
            .collect();
        self.thumbs.strip(&shown);
        self.thumbs.background(&self.order);
        if let (Some(folder_id), Some(root)) = (self.current_folder, self.root.clone()) {
            env.jobs.spawn(Lane::Query, "photos", move || {
                let result = catalog_ext::photos_in(&root, folder_id);
                Msg::Photos {
                    root,
                    folder_id,
                    result,
                }
            });
        }
    }

    fn can_add(&self) -> bool {
        self.busy.is_none() && self.importing.is_none()
    }
}

/// The side panel's width in Develop: the histogram and a margin.
fn histogram_panel_width() -> f32 {
    develop::histogram::WIDTH + 24.0
}

/// The next photo from `from` in the direction of `step` (+1 or -1) that is not `missing`.
pub fn next_openable(
    order: &[i64],
    from: i64,
    step: isize,
    missing: impl Fn(i64) -> bool,
) -> Option<i64> {
    let at = order.iter().position(|&id| id == from)?;
    let mut i = at as isize + step;
    while let Some(&id) = usize::try_from(i).ok().and_then(|i| order.get(i)) {
        if !missing(id) {
            return Some(id);
        }
        i += step;
    }
    None
}

/// "1,200 photos · 3 selected" (the selection part only when something is selected).
pub fn status_text(photos: usize, selected: usize) -> String {
    let noun = if photos == 1 { "photo" } else { "photos" };
    if selected == 0 {
        format!("{} {noun}", count(photos))
    } else {
        format!("{} {noun} · {} selected", count(photos), count(selected))
    }
}

/// The removal question's body: what is forgotten and what stays.
pub fn remove_folder_text(photos: usize) -> String {
    let noun = if photos == 1 { "photo" } else { "photos" };
    format!(
        "The catalogue forgets its {} {noun} and the edits it keeps for them. No files are \
         deleted: the photos and the edit files beside them stay where they are.",
        count(photos)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_text_counts_photos_and_selection() {
        assert_eq!(status_text(1_200, 3), "1,200 photos · 3 selected");
        assert_eq!(status_text(1_200, 0), "1,200 photos");
        assert_eq!(status_text(1, 1), "1 photo · 1 selected");
        assert!(remove_folder_text(2).contains("No files are deleted"));
    }

    #[test]
    fn graphics_banners_say_what_to_do() {
        assert_eq!(
            crate::app::DEVICE_LOST_BANNER,
            "The graphics device was reset. Restart lensflow-ImageEditor to continue."
        );
        assert_eq!(
            THUMBS_STOPPED_BANNER,
            "Thumbnails stopped because of a graphics error. They will restart when you \
             change folders."
        );
    }

    #[test]
    fn develop_arrows_step_over_missing_photos() {
        let order = [1, 2, 3, 4, 5];
        let missing = |id: i64| id == 2 || id == 4;
        assert_eq!(next_openable(&order, 3, 1, missing), Some(5));
        assert_eq!(next_openable(&order, 3, -1, missing), Some(1));
        assert_eq!(
            next_openable(&order, 5, 1, missing),
            None,
            "nothing after the last"
        );
        assert_eq!(next_openable(&order, 1, -1, missing), None);
        assert_eq!(next_openable(&order, 1, 1, |id| id != 5), Some(5));
        assert_eq!(
            next_openable(&order, 9, 1, missing),
            None,
            "not in the folder"
        );
    }

    /// Every library message is consumed by `Library::handle` when a library exists (the
    /// session flow's test lists who owns each variant, exhaustively); `JobFailed` is shared
    /// and comes back for the app's toast.
    #[test]
    fn handle_consumes_every_library_message() {
        use crate::jobs::Jobs;
        use crate::jobs::owners::{Owner, messages_owned_by};
        use crate::toast::Toasts;
        let Some(gpu) = lf_pipeline::test_support::try_shared_gpu() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let mut prefs = Prefs::default();
        let mut lib = Library::new(gpu, dir.path().join("frames"), &prefs);
        let jobs = Jobs::new(egui::Context::default());
        let mut toasts = Toasts::default();
        let config = dir.path().join("config");
        let mut env = Env {
            jobs: &jobs,
            toasts: &mut toasts,
            prefs: &mut prefs,
            config_dir: &config,
        };
        // Every library message (`every_message` holds one of each variant, checked against
        // strum's list; `owner()` picks the library's), and a current open answer.
        let mut messages = messages_owned_by(&[Owner::Library], 0, dir.path());
        messages.push(Msg::Opened {
            photo_id: 1,
            generation: 0,
            result: Err("current".into()),
        });
        for msg in messages {
            let shown = format!("{msg:?}");
            assert!(
                lib.handle(msg, &mut env).is_none(),
                "{shown} was handed back"
            );
        }
        let failed = Msg::JobFailed {
            name: "open photo".into(),
            message: "boom".into(),
        };
        assert!(matches!(
            lib.handle(failed, &mut env),
            Some(Msg::JobFailed { .. })
        ));
        assert!(matches!(
            lib.handle(Msg::Cleared, &mut env),
            Some(Msg::Cleared)
        ));
    }
}
