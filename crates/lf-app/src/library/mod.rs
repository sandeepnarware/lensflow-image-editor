//! The library: catalogue root, folders, the grid, the filmstrip and the selection.
//!
//! The UI thread holds no catalogue connection: quick reads (folders, a folder's photos) run
//! on the `Query` lane and writes (imports, rescans, removals) on the `Io` lane, each job with
//! its own `Catalog`, answering with a [`Msg`]. So a folder never waits behind an import to
//! show. The thumbnail pool has its own connections.

pub mod filmstrip;
pub mod grid;
pub mod import_ui;
pub mod selection;

use crate::app::DEVICE_LOST_BANNER;
use crate::catalog_ext::{self, count};
use crate::clipboard::{self, Clipboard};
use crate::develop::{self, Develop, Outcome};
use crate::dialogs::batch::{self, BatchAction, BatchDialog, BatchProgress, Choice};
use crate::dialogs::group_picker::{CopyAction, CopyDialog};
use crate::jobs::{Jobs, Lane, Msg};
use crate::prefs::Prefs;
use crate::theme;
use crate::thumbs_ui::{ThumbEvent, Thumbs};
use crate::toast::{Level, Toasts};
use eframe::egui::{self, Key, Modifiers, RichText};
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

/// The Edit menu's undo and redo, with their shortcuts (`Ctrl+Y` redoes too).
pub const UNDO: &str = "Undo";
pub const REDO: &str = "Redo";
pub const UNDO_KEY: egui::KeyboardShortcut =
    egui::KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
pub const REDO_KEY: egui::KeyboardShortcut =
    egui::KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Z);
/// Copy and paste of edits, and applying edits to the selection.
pub const COPY: &str = "Copy edits…";
pub const PASTE: &str = "Paste edits";
pub const APPLY: &str = "Apply to selected…";
pub const COPY_KEY: egui::KeyboardShortcut =
    egui::KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::C);
pub const PASTE_KEY: egui::KeyboardShortcut =
    egui::KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::V);
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
        }
    }

    /// egui's renderer, for the Develop preview's shared texture (none in headless tests).
    pub fn set_render_state(&mut self, render_state: Option<RenderState>) {
        self.develop.set_render_state(render_state);
    }

    /// The Develop view.
    pub fn develop(&self) -> &Develop {
        &self.develop
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

    /// Stops a running import after its current batch (the app is quitting).
    pub fn stop_import(&self) {
        self.stop_import.store(true, Ordering::SeqCst);
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

    /// Commits a gesture still in progress (the app is quitting).
    pub fn finish_edits(&mut self, toasts: &mut Toasts) {
        self.develop.finish_edit(toasts);
    }

    /// Handles the library's messages; anything else is handed back.
    pub fn handle(&mut self, msg: Msg, env: &mut Env) -> Option<Msg> {
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
            Msg::Folders(result) => match result {
                Ok(folders) => self.folders_loaded(folders, env),
                Err(e) => env
                    .toasts
                    .push(Level::Error, format!("Could not list the folders. {e}")),
            },
            Msg::Photos { folder_id, result } => {
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
            env.jobs.spawn(Lane::Query, "photos", move || Msg::Photos {
                folder_id,
                result: catalog_ext::photos_in(&root, folder_id),
            });
        }
    }

    fn refresh_folders(&self, env: &mut Env) {
        let Some(root) = self.root.clone() else {
            return;
        };
        env.jobs.spawn(Lane::Query, "folders", move || {
            Msg::Folders(catalog_ext::folders(&root))
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
        self.keys(ui.ctx(), env);
        self.sync_develop(env);
        self.develop.tick(ui.ctx(), env.jobs, env.toasts);

        if self.thumbs.stopped() && !self.gpu.is_lost() {
            egui::Panel::top("library-device").show(ui, |ui| {
                ui.add_space(4.0);
                ui.label(RichText::new(DEVICE_LOST_BANNER).color(theme::DANGER));
                ui.add_space(4.0);
            });
        }
        egui::Panel::top("library-menu").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.menu_button("File", |ui| {
                    if ui
                        .add_enabled(self.can_add(), egui::Button::new("Add folder…"))
                        .clicked()
                    {
                        self.add_folder(env);
                    }
                });
                ui.menu_button("Edit", |ui| {
                    let editing = matches!(self.mode, Mode::Develop(_));
                    let undo =
                        egui::Button::new(UNDO).shortcut_text(ui.ctx().format_shortcut(&UNDO_KEY));
                    if ui
                        .add_enabled(editing && self.develop.can_undo(), undo)
                        .clicked()
                    {
                        self.develop.undo(env.toasts);
                    }
                    let redo =
                        egui::Button::new(REDO).shortcut_text(ui.ctx().format_shortcut(&REDO_KEY));
                    if ui
                        .add_enabled(editing && self.develop.can_redo(), redo)
                        .clicked()
                    {
                        self.develop.redo(env.toasts);
                    }
                    ui.separator();
                    let copy =
                        egui::Button::new(COPY).shortcut_text(ui.ctx().format_shortcut(&COPY_KEY));
                    if ui.add_enabled(self.copy_source().is_some(), copy).clicked() {
                        self.copying = true;
                    }
                    let paste = egui::Button::new(PASTE)
                        .shortcut_text(ui.ctx().format_shortcut(&PASTE_KEY));
                    if ui
                        .add_enabled(!self.selection.current.is_empty(), paste)
                        .clicked()
                    {
                        self.paste(ui.ctx(), env);
                    }
                    if ui
                        .add_enabled(!self.selection.current.is_empty(), egui::Button::new(APPLY))
                        .clicked()
                    {
                        self.open_batch(None);
                    }
                });
            });
        });
        egui::Panel::bottom("library-status")
            .exact_size(24.0)
            .show(ui, |ui| self.status(ui));
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
        self.reveal_active = false;
    }

    fn keys(&mut self, ctx: &egui::Context, env: &mut Env) {
        if ctx.egui_wants_keyboard_input()
            || crate::dialogs::modal_open(ctx)
            || self.confirm_remove.is_some()
            || self.confirm_missing.is_some()
            || self.adding
            || self.copying
            || self.batch.is_some()
        {
            return;
        }
        let (copy, paste) = ctx.input_mut(|i| {
            (
                i.consume_shortcut(&COPY_KEY),
                i.consume_shortcut(&PASTE_KEY),
            )
        });
        if copy && self.copy_source().is_some() {
            self.copying = true;
        }
        if paste {
            self.paste(ctx, env);
        }
        if matches!(self.mode, Mode::Develop(_)) {
            // Ctrl+Shift+Z first: Ctrl+Z also matches it.
            let (redo, undo) = ctx.input_mut(|i| {
                let redo =
                    i.consume_shortcut(&REDO_KEY) || i.consume_key(Modifiers::COMMAND, Key::Y);
                (redo, i.consume_shortcut(&UNDO_KEY))
            });
            if redo {
                self.develop.redo(env.toasts);
            }
            if undo {
                self.develop.undo(env.toasts);
            }
        }
        let (left, right, up, down, all, develop, library) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowLeft),
                i.consume_key(Modifiers::NONE, Key::ArrowRight),
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::COMMAND, Key::A),
                i.consume_key(Modifiers::NONE, Key::D),
                i.consume_key(Modifiers::NONE, Key::G),
            )
        });
        let in_grid = self.mode == Mode::Library;
        let moves = [
            (left, Dir::Left),
            (right, Dir::Right),
            (up && in_grid, Dir::Up),
            (down && in_grid, Dir::Down),
        ];
        for (pressed, dir) in moves {
            if !pressed {
                continue;
            }
            if let Mode::Develop(current) = self.mode {
                // Develop steps over missing photos to the next one it can open.
                let step = if dir == Dir::Left { -1 } else { 1 };
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
        if all {
            // The filmstrip in Develop too: the open photo stays the active one.
            self.selection.select_all(&self.order);
        }
        if develop
            && in_grid
            && let Some(id) = self.selection.active()
        {
            self.open_photo(id, env);
        }
        if library {
            self.mode = Mode::Library;
            self.reveal_active = true;
        }
    }

    fn status(&self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            ui.label(status_text(self.photos.len(), self.selection.current.len()));
            if let Some(text) = self.busy {
                ui.separator();
                ui.label(RichText::new(text).color(theme::MUTED));
            }
            if let Some(importing) = &self.importing {
                ui.separator();
                ui.label(importing.text());
                if let Some(p) = importing.progress.filter(|p| p.total > 0) {
                    ui.add(
                        egui::ProgressBar::new(p.done as f32 / p.total as f32).desired_width(160.0),
                    );
                }
            }
        });
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
                        if ui.button("Remove from catalogue…").clicked() {
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
        for id in ids {
            self.thumbs.forget(*id);
        }
        self.thumbs.background(&self.order);
        if let Mode::Develop(open) = self.mode
            && ids.contains(&open)
        {
            self.develop.reload_edit(env.toasts);
        }
        if let (Some(folder_id), Some(root)) = (self.current_folder, self.root.clone()) {
            env.jobs.spawn(Lane::Query, "photos", move || Msg::Photos {
                folder_id,
                result: catalog_ext::photos_in(&root, folder_id),
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
