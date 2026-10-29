//! Choosing folders (the catalogue root on first run, and folders to add), opening the
//! catalogue, and importing.
//!
//! A folder comes from the system picker or from a typed path. Each picker runs on its own
//! short-lived thread, never on a lane: a picker that fails says nothing (rfd answers `None`
//! for a cancel and a failure alike), and one that never answers must not hold up other
//! work. A typed path is always available, so a machine without a working picker (no
//! desktop portal and no zenity on Linux) can still choose a folder. Scans and imports run
//! on the `Long` lane; opening the catalogue on the `Io` lane.

use super::{Env, Library, Stage};
use crate::catalog_ext::{self, Imported, NETWORK_WARNING, count};
use crate::jobs::{Lane, Msg, Reporter};
use crate::theme;
use crate::toast::Level;
use eframe::egui::{self, Key, RichText};
use lf_catalog::Folder;
use lf_catalog::import::{self, Progress, Scan};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

/// The first-run screen's text.
pub const FIRST_RUN: &str = "Choose a folder for your catalogue. Photos stay where they are; the catalogue only remembers them.";
/// After Preferences → Change…: go back to the catalogue in use.
pub const KEEP_ROOT: &str = "Keep the current catalogue";
/// The label of the typed-path field.
pub const TYPE_PATH: &str = "Or type a folder path";

/// What a chosen folder is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// The catalogue root.
    Root,
    /// A folder of photos to add.
    AddFolder,
    /// The export dialog's destination (checked writable too); its own tickets.
    ExportTo,
    /// The export dialog's watermark logo (a picture file); its own tickets.
    Watermark,
    /// The export dialog's ICC profile (a file); its own tickets.
    ExportProfile,
    /// A `.cube` file for Develop's LUT look.
    LutFile,
    /// A `.dcp` camera profile for Develop's RAW section.
    DcpFile,
    /// Replace sky's sky photo.
    SkyFile,
    /// The printer and paper profile for soft-proofing.
    ProofProfile,
    /// The application "Edit in external editor" uses.
    Editor,
    /// Where Print's "Save PDF" writes.
    PrintPdf,
    /// One of "Import from a card"'s folders; its own tickets.
    Ingest(crate::dialogs::ingest::Pick),
    /// Auto import's watched folder; the dialog's own tickets.
    AutoImportWatch,
}

/// A folder that exists and can be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chosen {
    pub path: PathBuf,
    /// It is on a network drive (asked about for the catalogue root).
    pub network: bool,
}

/// The folder chooser's state: the typed path, the open picker and the last problem.
#[derive(Debug, Default)]
pub struct Chooser {
    /// Increases with every choice started; older answers are ignored.
    ticket: u64,
    /// The ticket of the picker still open, if any.
    picker: Option<u64>,
    /// A typed path is being checked.
    checking: bool,
    typed: String,
    error: Option<String>,
}

/// A folder being scanned, then imported: it cannot be removed until the import ends.
#[derive(Debug, Clone)]
pub struct Importing {
    pub folder: PathBuf,
    pub progress: Option<Progress>,
    /// "Add folder…" (shown when its import ends), not a rescan.
    pub added: bool,
}

impl Importing {
    /// "Importing 240 of 1,200" for the status bar.
    pub fn text(&self) -> String {
        match self.progress {
            Some(p) => import_text(p.done, p.total),
            None => import_text(0, 0),
        }
    }
}

/// "Importing 240 of 1,200" (or "Importing…" before the first count).
pub fn import_text(done: usize, total: usize) -> String {
    if total == 0 {
        "Importing…".to_owned()
    } else {
        format!("Importing {} of {}", count(done), count(total))
    }
}

/// The toast at the end of an import.
pub fn summary_text(imported: &Imported) -> String {
    let s = imported.summary;
    let mut text = format!(
        "Import finished: {} added, {} updated, {} unchanged",
        count(s.added),
        count(s.updated),
        count(s.skipped)
    );
    if imported.missing > 0 {
        text.push_str(&format!(", {} missing", count(imported.missing)));
    }
    text.push('.');
    text
}

/// The question asked when a readable folder holds none of its photos any more.
pub fn emptied_question(photos: usize) -> String {
    if photos == 1 {
        "The photo is not in this folder any more. Mark it missing?".to_owned()
    } else {
        format!(
            "None of the {} photos are in this folder any more. Mark them missing?",
            count(photos)
        )
    }
}

/// Runs `choose` on its own thread and sends what it returns as the answer to `ticket`.
/// A panic answers with an error instead of leaving the chooser waiting.
pub(crate) fn on_own_thread(
    reporter: Reporter,
    purpose: Purpose,
    ticket: u64,
    choose: impl FnOnce() -> Result<Option<Chosen>, String> + Send + 'static,
) {
    let spawned = std::thread::Builder::new()
        .name("lf-folder-choice".to_owned())
        .spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(choose)).unwrap_or_else(|_| {
                log::error!("choosing a folder ({purpose:?}) crashed");
                Err(
                    "The folder picker stopped unexpectedly. Type the folder path instead."
                        .to_owned(),
                )
            });
            reporter.send(Msg::FolderChosen {
                purpose,
                ticket,
                result,
            });
        });
    if let Err(e) = spawned {
        log::error!("could not start the folder chooser: {e}");
    }
}

impl Library {
    /// The chooser's current ticket (tests use it to answer as a picker would).
    pub fn ticket(&self) -> u64 {
        self.chooser.ticket
    }

    /// Replaces the typed folder path (tests use it).
    pub fn set_typed_path(&mut self, path: &str) {
        self.chooser.typed = path.to_owned();
    }

    /// The picker button, the typed-path field and the last problem.
    fn chooser_ui(&mut self, ui: &mut egui::Ui, purpose: Purpose, env: &mut Env) {
        let picking = self.chooser.picker.is_some();
        let text = if picking {
            "Waiting for the folder picker…"
        } else {
            "Choose a folder…"
        };
        if ui.add_enabled(!picking, egui::Button::new(text)).clicked() {
            self.open_picker(purpose, env);
        }
        ui.add_space(12.0);
        let label = ui.label(TYPE_PATH);
        let field = ui
            .add(egui::TextEdit::singleline(&mut self.chooser.typed).desired_width(360.0))
            .labelled_by(label.id);
        let entered = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        let can_use = !self.chooser.checking && !self.chooser.typed.trim().is_empty();
        let clicked = ui
            .add_enabled(can_use, egui::Button::new("Use this folder"))
            .clicked();
        if clicked || (entered && can_use) {
            self.check_typed(purpose, env);
        }
        if let Some(error) = &self.chooser.error {
            ui.label(RichText::new(error).color(theme::DANGER));
        }
    }

    fn open_picker(&mut self, purpose: Purpose, env: &mut Env) {
        self.chooser.ticket += 1;
        self.chooser.error = None;
        self.chooser.checking = false;
        let ticket = self.chooser.ticket;
        self.chooser.picker = Some(ticket);
        let title = match purpose {
            Purpose::Root => "Choose a folder for your catalogue",
            Purpose::AddFolder => "Add a folder of photos",
            Purpose::ExportTo => "Export to this folder",
            Purpose::Watermark => "Choose a logo for the watermark",
            Purpose::ExportProfile => "Choose an ICC profile",
            Purpose::Ingest(_) | Purpose::AutoImportWatch => "Choose a folder",
            Purpose::LutFile => "Choose a .cube LUT",
            Purpose::DcpFile => "Choose a .dcp camera profile",
            Purpose::SkyFile => "Choose a photo of a sky",
            Purpose::ProofProfile => "Choose a printer and paper profile",
            Purpose::Editor => "Choose an application",
            Purpose::PrintPdf => "Save the PDF",
        };
        on_own_thread(env.jobs.reporter(), purpose, ticket, move || {
            let picked = rfd::FileDialog::new().set_title(title).pick_folder();
            log::info!("folder picker ({purpose:?}) answered: {picked:?}");
            Ok(picked.map(|path| Chosen {
                network: catalog_ext::is_network_path(&path),
                path,
            }))
        });
    }

    fn check_typed(&mut self, purpose: Purpose, env: &mut Env) {
        self.chooser.ticket += 1;
        self.chooser.error = None;
        self.chooser.checking = true;
        let ticket = self.chooser.ticket;
        let path = PathBuf::from(self.chooser.typed.trim());
        on_own_thread(env.jobs.reporter(), purpose, ticket, move || {
            catalog_ext::check_folder(&path).map_err(str::to_owned)?;
            log::info!("typed folder ({purpose:?}): {}", path.display());
            Ok(Some(Chosen {
                network: catalog_ext::is_network_path(&path),
                path,
            }))
        });
    }

    pub(super) fn folder_chosen(
        &mut self,
        purpose: Purpose,
        ticket: u64,
        result: Result<Option<Chosen>, String>,
        env: &mut Env,
    ) {
        if purpose == Purpose::ExportTo {
            if let Some(dialog) = &mut self.export {
                dialog.folder_chosen(ticket, result.map(|c| c.map(|c| c.path)));
            }
            return;
        }
        if purpose == Purpose::PrintPdf {
            match result {
                Ok(chosen) => self.print_save_chosen(chosen.map(|c| c.path), env),
                Err(e) => env.toasts.push(Level::Warning, e),
            }
            return;
        }
        if purpose == Purpose::Editor {
            match result {
                Ok(Some(chosen)) => self.editor_chosen(chosen.path, env),
                Ok(None) => {}
                Err(e) => env.toasts.push(Level::Warning, e),
            }
            return;
        }
        if purpose == Purpose::ProofProfile {
            match result {
                Ok(Some(chosen)) => {
                    env.prefs.proof_profile = Some(chosen.path.clone());
                    env.save_prefs();
                    self.develop.set_proof(Some(chosen.path));
                }
                Ok(None) => {}
                Err(e) => env.toasts.push(Level::Warning, e),
            }
            return;
        }
        if purpose == Purpose::LutFile {
            match result {
                Ok(Some(chosen)) => self.import_lut(chosen.path, env),
                Ok(None) => {}
                Err(e) => env.toasts.push(Level::Warning, e),
            }
            return;
        }
        if purpose == Purpose::SkyFile {
            match result {
                Ok(Some(chosen)) => {
                    if let Some(dialog) = &mut self.ai {
                        dialog.sky = Some(chosen.path);
                    }
                }
                Ok(None) => {}
                Err(e) => env.toasts.push(Level::Warning, e),
            }
            return;
        }
        if purpose == Purpose::DcpFile {
            match result {
                Ok(Some(chosen)) => self.import_dcp(chosen.path, env),
                Ok(None) => {}
                Err(e) => env.toasts.push(Level::Warning, e),
            }
            return;
        }
        if purpose == Purpose::AutoImportWatch {
            match result {
                Ok(chosen) => self.auto_import_watch_chosen(ticket, chosen.map(|c| c.path)),
                Err(e) => env.toasts.push(Level::Warning, e),
            }
            return;
        }
        if let Purpose::Ingest(pick) = purpose {
            match result {
                Ok(chosen) => self.ingest_picked(pick, ticket, chosen.map(|c| c.path), env),
                Err(e) => env.toasts.push(Level::Warning, e),
            }
            return;
        }
        if purpose == Purpose::ExportProfile {
            if let Some(dialog) = &mut self.export {
                dialog.profile_chosen(ticket, result.map(|c| c.map(|c| c.path)));
            }
            return;
        }
        if purpose == Purpose::Watermark {
            if let Some(dialog) = &mut self.export {
                dialog.logo_chosen(ticket, result.map(|c| c.map(|c| c.path)));
            }
            return;
        }
        if self.chooser.picker == Some(ticket) {
            self.chooser.picker = None;
        }
        if ticket != self.chooser.ticket {
            return; // superseded by a later choice
        }
        self.chooser.checking = false;
        let chosen = match result {
            Ok(Some(chosen)) => chosen,
            Ok(None) => return, // the picker was cancelled
            Err(message) => {
                self.chooser.error = Some(message);
                return;
            }
        };
        self.chooser.typed.clear();
        match purpose {
            Purpose::Root if chosen.network => self.stage = Stage::NetworkWarning(chosen.path),
            Purpose::Root => self.use_root(chosen.path, env),
            Purpose::ExportTo
            | Purpose::Watermark
            | Purpose::ExportProfile
            | Purpose::Ingest(_)
            | Purpose::AutoImportWatch
            | Purpose::LutFile
            | Purpose::DcpFile
            | Purpose::SkyFile
            | Purpose::ProofProfile
            | Purpose::Editor
            | Purpose::PrintPdf => {}
            Purpose::AddFolder => {
                if self.adding {
                    self.adding = false;
                    self.scan_folder(chosen.path, true, env);
                }
            }
        }
    }

    /// The first-run, network-warning, opening and failure screens.
    pub(super) fn root_screen(&mut self, ui: &mut egui::Ui, env: &mut Env) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() / 4.0);
            match self.stage.clone() {
                Stage::NoRoot => {
                    ui.label(RichText::new(FIRST_RUN).size(16.0));
                    ui.add_space(12.0);
                    self.chooser_ui(ui, Purpose::Root, env);
                    if let Some(keep) = self.keep.clone() {
                        ui.add_space(12.0);
                        ui.label(RichText::new(keep.display().to_string()).color(theme::MUTED));
                        if ui.button(KEEP_ROOT).clicked() {
                            self.use_root(keep, env);
                        }
                    }
                }
                Stage::NetworkWarning(path) => {
                    ui.label(RichText::new(NETWORK_WARNING).color(theme::WARNING));
                    ui.label(RichText::new(path.display().to_string()).color(theme::MUTED));
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui.button("Choose another").clicked() {
                            self.stage = Stage::NoRoot;
                            self.open_picker(Purpose::Root, env);
                        }
                        if ui.button("Use it anyway").clicked() {
                            log::warn!("catalogue root on a network drive: {}", path.display());
                            self.use_root(path, env);
                        }
                    });
                }
                Stage::Closed | Stage::Opening => {
                    ui.spinner();
                    ui.label("Opening the catalogue…");
                }
                Stage::Failed(message) => {
                    ui.label(
                        RichText::new(format!("Could not open the catalogue. {message}"))
                            .color(theme::DANGER),
                    );
                    ui.add_space(12.0);
                    ui.label("Choose another folder for your catalogue.");
                    ui.add_space(8.0);
                    self.chooser_ui(ui, Purpose::Root, env);
                }
                Stage::Ready => {}
            }
        });
    }

    fn use_root(&mut self, path: PathBuf, env: &mut Env) {
        self.keep = None;
        env.prefs.catalogue_root = Some(path.clone());
        env.save_prefs();
        self.root = Some(path);
        self.folders.clear();
        self.current_folder = None;
        self.loading = None;
        self.photos.clear();
        self.rows.clear();
        self.order.clear();
        self.index.clear();
        self.selection = Default::default();
        self.frames = None;
        self.stage = Stage::Closed;
        self.open_catalogue(env);
    }

    pub(super) fn open_catalogue(&mut self, env: &mut Env) {
        let Some(root) = self.root.clone() else {
            self.stage = Stage::NoRoot;
            return;
        };
        self.stage = Stage::Opening;
        let gpu = self.gpu.clone();
        let frames_dir = self.frames_dir.clone();
        let cap = env.prefs.frame_cache_bytes;
        env.jobs.spawn(Lane::Io, "open catalogue", move || {
            let result = catalog_ext::open(gpu, &root, &frames_dir, cap).map(Box::new);
            Msg::CatalogueOpened { root, result }
        });
    }

    /// Opens the "Add folder" dialog (picker and typed path).
    pub(super) fn add_folder(&mut self, env: &mut Env) {
        if !self.can_add() {
            return;
        }
        self.adding = true;
        self.chooser.error = None;
        self.revive_thumbs(env);
    }

    pub(super) fn add_folder_dialog(&mut self, ctx: &egui::Context, env: &mut Env) {
        if !self.adding {
            return;
        }
        let mut cancel = false;
        egui::Modal::new(egui::Id::unique("add-folder")).show(ctx, |ui| {
            ui.set_max_width(440.0);
            ui.heading("Add a folder of photos");
            ui.add_space(4.0);
            ui.label(
                RichText::new("Photos stay where they are; the catalogue only remembers them.")
                    .color(theme::MUTED),
            );
            ui.add_space(12.0);
            self.chooser_ui(ui, Purpose::AddFolder, env);
            ui.add_space(12.0);
            if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                cancel = true;
            }
        });
        if cancel {
            self.adding = false;
            // An answer still on its way is ignored.
            self.chooser.ticket += 1;
            self.chooser.checking = false;
            self.chooser.error = None;
        }
    }

    pub(super) fn rescan(&mut self, folder: &Folder, env: &mut Env) {
        if !self.can_add() {
            return;
        }
        self.revive_thumbs(env);
        self.scan_folder(folder.path.clone(), false, env);
    }

    /// Scans `folder` on the `Long` lane (`added`: for "Add folder…", else a rescan). The
    /// folder counts as importing from here on.
    pub(super) fn scan_folder(&mut self, folder: PathBuf, added: bool, env: &mut Env) {
        self.busy = Some("Looking for photos…");
        self.importing = Some(Importing {
            folder: folder.clone(),
            progress: None,
            added,
        });
        let name = if added { "add folder" } else { "rescan" };
        env.jobs.spawn(Lane::Long, name, move || {
            let scan = import::scan(&folder);
            Msg::Scanned { folder, scan }
        });
    }

    pub(super) fn scanned(&mut self, folder: PathBuf, scan: Scan, env: &mut Env) {
        self.busy = None;
        let added = self
            .importing
            .take()
            .is_some_and(|i| i.added && i.folder == folder);
        if !scan.root_readable {
            env.toasts.push(
                Level::Error,
                format!(
                    "Could not read {}. Check that the drive is connected, then rescan.",
                    folder.display()
                ),
            );
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        log::info!(
            "importing {} ({} files found)",
            folder.display(),
            scan.candidates.len()
        );
        self.importing = Some(Importing {
            folder: folder.clone(),
            progress: Some(Progress {
                done: 0,
                total: scan.candidates.len(),
            }),
            added,
        });
        let reporter = env.jobs.reporter();
        let stop = self.stop_import.clone();
        env.jobs.spawn(Lane::Long, "import", move || {
            let result = catalog_ext::import_folder(&root, &folder, &scan, &stop, &mut |p| {
                reporter.send(Msg::ImportProgress(p));
            });
            Msg::ImportDone(result)
        });
    }

    pub(super) fn import_done(&mut self, result: Result<Imported, String>, env: &mut Env) {
        let importing = self.importing.take();
        let added = importing.as_ref().is_some_and(|i| i.added);
        let folder = importing.map(|i| i.folder);
        let shown = folder.as_deref().map_or_else(String::new, display);
        match result {
            Ok(imported) => {
                let s = imported.summary;
                log::info!(
                    "import of {shown} finished: {} added, {} updated, {} unchanged, {} newly missing",
                    s.added,
                    s.updated,
                    s.skipped,
                    imported.missing
                );
                env.toasts.push(Level::Success, summary_text(&imported));
                if let Some(n) = imported.emptied {
                    log::info!("{shown} was read and holds none of its {n} photos; asking");
                    self.confirm_missing = Some((imported.folder_id, n));
                }
                // A folder just added is shown in the library; after a rescan the view
                // moves to the imported folder only when nothing else is shown. A
                // photographer in Develop stays where they are.
                let here = self.current_folder == Some(imported.folder_id);
                let in_library = self.mode == super::Mode::Library;
                let nothing_shown = self.current_folder.is_none() && in_library;
                self.refresh_folders(env);
                if here || nothing_shown || (added && in_library) {
                    self.show_folder(Some(imported.folder_id), env);
                }
            }
            Err(e) => {
                log::error!("import of {shown} failed: {e}");
                env.toasts
                    .push(Level::Error, format!("The import of {shown} stopped. {e}"));
                self.refresh_folders(env);
                if let Some(current) = self.current_folder {
                    self.show_folder(Some(current), env);
                }
            }
        }
    }

    /// "None of the N photos are in this folder any more. Mark them missing?"
    pub(super) fn confirm_missing_dialog(&mut self, ctx: &egui::Context, env: &mut Env) {
        let Some((folder_id, photos)) = self.confirm_missing else {
            return;
        };
        let (mut mark, mut keep) = (false, false);
        egui::Modal::new(egui::Id::unique("confirm-mark-missing")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.label(emptied_question(photos));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                mark = ui.button("Mark missing").clicked();
                keep = ui.button("Keep them").clicked() || ui.input(|i| i.key_pressed(Key::Escape));
            });
        });
        if mark && let Some(root) = self.root.clone() {
            env.jobs
                .spawn(Lane::Io, "mark missing", move || Msg::MarkedMissing {
                    folder_id,
                    result: catalog_ext::mark_missing_confirmed(&root, folder_id),
                });
        }
        if mark || keep {
            self.confirm_missing = None;
        }
    }

    pub(super) fn marked_missing(
        &mut self,
        folder_id: i64,
        result: Result<usize, String>,
        env: &mut Env,
    ) {
        match result {
            Ok(n) => {
                log::info!("marked {n} photos of folder {folder_id} missing");
                if self.current_folder == Some(folder_id) {
                    self.show_folder(Some(folder_id), env);
                }
            }
            Err(e) => env.toasts.push(
                Level::Error,
                format!("Could not mark the photos missing. {e}"),
            ),
        }
    }
}

fn display(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lf_catalog::import::Summary;

    #[test]
    fn import_texts_read_well() {
        assert_eq!(import_text(240, 1_200), "Importing 240 of 1,200");
        assert_eq!(import_text(0, 0), "Importing…");
        let imported = Imported {
            folder_id: 1,
            summary: Summary {
                added: 48,
                updated: 2,
                skipped: 1_000,
            },
            missing: 3,
            emptied: None,
        };
        assert_eq!(
            summary_text(&imported),
            "Import finished: 48 added, 2 updated, 1,000 unchanged, 3 missing."
        );
        assert_eq!(
            emptied_question(1_200),
            "None of the 1,200 photos are in this folder any more. Mark them missing?"
        );
        assert_eq!(
            emptied_question(1),
            "The photo is not in this folder any more. Mark it missing?"
        );
    }
}
