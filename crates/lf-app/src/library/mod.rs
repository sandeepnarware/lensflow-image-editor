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
use crate::jobs::{Jobs, Lane, Msg};
use crate::prefs::Prefs;
use crate::theme;
use crate::thumbs_ui::{ThumbEvent, Thumbs};
use crate::toast::{Level, Toasts};
use eframe::egui::{self, Key, Modifiers, RichText};
use lf_catalog::{Folder, PhotoRow};
use lf_pipeline::Gpu;
use lf_raw::cache::FrameCache;
use selection::{Dir, Selection};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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
    /// Develop (Task 4) with this photo.
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
    frames: Option<FrameCache>,
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
}

impl Library {
    pub fn new(gpu: Arc<Gpu>, frames_dir: PathBuf, prefs: &Prefs) -> Library {
        let root = prefs.catalogue_root.clone();
        Library {
            gpu,
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
        }
    }

    /// The catalogue root in use.
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// The decoded-frame cache, once the catalogue is open (Develop uses it).
    pub fn frames(&self) -> Option<&FrameCache> {
        self.frames.as_ref()
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
            Msg::JobFailed { name, message } => {
                // A panicking library job must not leave the library waiting for it.
                match name.as_str() {
                    "add folder" | "rescan" => self.busy = None,
                    "photos" => self.loading = None,
                    "import" => self.importing = None,
                    "open catalogue" => self.stage = Stage::Failed(message.clone()),
                    "thumbnail pool" => self.reviving = false,
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
                self.frames = Some(opened.frames);
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
        egui::CentralPanel::default_margins().show(ui, |ui| match self.mode {
            Mode::Library => self.grid(ui, env),
            Mode::Develop(id) => self.develop_placeholder(ui, id),
        });
        self.confirm_remove_dialog(ui.ctx(), env);
        self.add_folder_dialog(ui.ctx(), env);
        self.confirm_missing_dialog(ui.ctx(), env);
        self.reveal_active = false;
    }

    fn keys(&mut self, ctx: &egui::Context, env: &mut Env) {
        if ctx.egui_wants_keyboard_input()
            || self.confirm_remove.is_some()
            || self.confirm_missing.is_some()
            || self.adding
        {
            return;
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
            if pressed {
                self.selection.arrow(dir, self.columns, &self.order);
                self.reveal_active = true;
                if let Mode::Develop(_) = self.mode
                    && let Some(id) = self.selection.active()
                {
                    self.open_photo(id, env);
                }
            }
        }
        if all && in_grid {
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

    fn develop_placeholder(&mut self, ui: &mut egui::Ui, id: i64) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() / 4.0);
            if let Some((texture, size)) = self.thumbs.texture(id) {
                let scale = (512.0 / size.x.max(size.y)).max(1.0);
                ui.image(egui::load::SizedTexture::new(texture, size * scale));
                ui.add_space(12.0);
            }
            if let Some(photo) = self.photo(id) {
                ui.label(photo.path.display().to_string());
            }
            ui.label(
                RichText::new("Editing arrives in the next version. Press G for the library.")
                    .color(theme::MUTED),
            );
        });
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

    fn can_add(&self) -> bool {
        self.busy.is_none() && self.importing.is_none()
    }
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
}
