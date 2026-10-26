//! Choosing the catalogue root (first run, the network-drive warning), opening it, and adding
//! or rescanning folders. The pickers, scans and imports all run on the `Io` lane.

use super::{Env, Library, Stage};
use crate::catalog_ext::{self, Imported, NETWORK_WARNING, count};
use crate::jobs::{Lane, Msg};
use crate::theme;
use crate::toast::Level;
use eframe::egui::{self, RichText};
use lf_catalog::Folder;
use lf_catalog::import::{self, Progress, Scan};
use std::path::{Path, PathBuf};

/// The first-run screen's text.
pub const FIRST_RUN: &str = "Choose a folder for your catalogue. Photos stay where they are; the catalogue only remembers them.";

/// A running import.
#[derive(Debug, Clone)]
pub struct Importing {
    pub folder: PathBuf,
    pub progress: Option<Progress>,
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

impl Library {
    /// The first-run, network-warning, opening and failure screens.
    pub(super) fn root_screen(&mut self, ui: &mut egui::Ui, env: &mut Env) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() / 3.0);
            match self.stage.clone() {
                Stage::NoRoot | Stage::Picking => {
                    ui.label(RichText::new(FIRST_RUN).size(16.0));
                    ui.add_space(12.0);
                    let picking = self.stage == Stage::Picking;
                    let text = if picking {
                        "Waiting for the folder picker…"
                    } else {
                        "Choose a folder…"
                    };
                    if ui.add_enabled(!picking, egui::Button::new(text)).clicked() {
                        self.pick_root(env);
                    }
                }
                Stage::NetworkWarning(path) => {
                    ui.label(RichText::new(NETWORK_WARNING).color(theme::WARNING));
                    ui.label(RichText::new(path.display().to_string()).color(theme::MUTED));
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui.button("Choose another").clicked() {
                            self.pick_root(env);
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
                    if ui.button("Choose another folder").clicked() {
                        self.pick_root(env);
                    }
                }
                Stage::Ready => {}
            }
        });
    }

    fn pick_root(&mut self, env: &mut Env) {
        self.stage = Stage::Picking;
        // Always on a lane: GTK dialogs must not run on winit's thread on Linux.
        env.jobs
            .spawn(Lane::Io, "choose catalogue folder", move || {
                let picked = rfd::FileDialog::new()
                    .set_title("Choose a folder for your catalogue")
                    .pick_folder()
                    .map(|path| {
                        let network = catalog_ext::is_network_path(&path);
                        (path, network)
                    });
                Msg::RootPicked(picked)
            });
    }

    pub(super) fn root_picked(&mut self, picked: Option<(PathBuf, bool)>, env: &mut Env) {
        match picked {
            // Cancelled: back to the remembered root (tried again), or the first-run screen.
            None if self.root.is_some() => self.stage = Stage::Closed,
            None => self.stage = Stage::NoRoot,
            Some((path, true)) => self.stage = Stage::NetworkWarning(path),
            Some((path, false)) => self.use_root(path, env),
        }
    }

    fn use_root(&mut self, path: PathBuf, env: &mut Env) {
        env.prefs.catalogue_root = Some(path.clone());
        env.save_prefs();
        self.root = Some(path);
        self.folders.clear();
        self.current_folder = None;
        self.photos.clear();
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

    pub(super) fn add_folder(&mut self, env: &mut Env) {
        if !self.can_add() {
            return;
        }
        self.busy = Some("Choose a folder of photos…");
        self.revive_thumbs(env);
        env.jobs
            .spawn(Lane::Io, "add folder", move || match rfd::FileDialog::new()
                .set_title("Add a folder of photos")
                .pick_folder()
            {
                None => Msg::NothingPicked,
                Some(folder) => {
                    let scan = import::scan(&folder);
                    Msg::Scanned { folder, scan }
                }
            });
    }

    pub(super) fn rescan(&mut self, folder: &Folder, env: &mut Env) {
        if !self.can_add() {
            return;
        }
        self.busy = Some("Looking for photos…");
        self.revive_thumbs(env);
        let path = folder.path.clone();
        env.jobs.spawn(Lane::Io, "rescan", move || {
            let scan = import::scan(&path);
            Msg::Scanned { folder: path, scan }
        });
    }

    pub(super) fn scanned(&mut self, folder: PathBuf, scan: Scan, env: &mut Env) {
        self.busy = None;
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
        });
        let reporter = env.jobs.reporter();
        let stop = self.stop_import.clone();
        env.jobs.spawn(Lane::Io, "import", move || {
            let result = catalog_ext::import_folder(&root, &folder, &scan, &stop, &mut |p| {
                reporter.send(Msg::ImportProgress(p));
            });
            Msg::ImportDone(result)
        });
    }

    pub(super) fn import_done(&mut self, result: Result<Imported, String>, env: &mut Env) {
        let folder = self.importing.take().map(|i| i.folder);
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
                self.mode = super::Mode::Library;
                self.refresh_folders(env);
                self.show_folder(Some(imported.folder_id), env);
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
        };
        assert_eq!(
            summary_text(&imported),
            "Import finished: 48 added, 2 updated, 1,000 unchanged, 3 missing."
        );
    }
}
