//! "Import Lightroom presets…" on the library's side: the dialog, its pickers (each on its own
//! short-lived thread, as the other pickers: one that never answers must not hold up a lane),
//! and the import on the `Long` lane ([`crate::lightroom::import::run`], its own catalogue
//! connection). The imported presets are read again and a sync is asked for, as a saved
//! preset does.

use super::{Env, Library};
use crate::dialogs::lr_import::{self, LrImportAction, LrImportDialog, Stage};
use crate::jobs::{Lane, Msg};
use crate::lightroom::import::ImportReport;
use crate::toast::Level;
use eframe::egui;
use std::path::PathBuf;

/// The job's name (a panic in it is reported under this name).
pub const JOB: &str = "import Lightroom presets";

impl Library {
    /// Whether "Import Lightroom presets…" can open: where "Save preset…" can (a studio is
    /// signed in to) and with a catalogue open.
    pub(super) fn can_import_lr(&self) -> bool {
        self.develop.studio().is_some() && self.root.is_some()
    }

    /// The dialog, while open (tests read its id to answer as a picker would).
    pub fn lr_import(&self) -> Option<&LrImportDialog> {
        self.lr_import.as_ref()
    }

    pub(super) fn open_lr_import(&mut self) {
        if self.lr_import.is_none() {
            self.lr_import = Some(LrImportDialog::new(
                crate::lightroom::gather::adobe_folders(),
            ));
        }
    }

    pub(super) fn lr_import_dialog(&mut self, ctx: &egui::Context, env: &mut Env) {
        let Some(dialog) = &mut self.lr_import else {
            return;
        };
        match dialog.show(ctx) {
            LrImportAction::None => {}
            LrImportAction::Close => self.lr_import = None,
            LrImportAction::ChooseFiles => pick(dialog.id, false, env),
            LrImportAction::ChooseFolder => pick(dialog.id, true, env),
            LrImportAction::Import(paths) => self.start_lr_import(paths, env),
        }
    }

    fn start_lr_import(&mut self, paths: Vec<PathBuf>, env: &mut Env) {
        let (Some(root), Some(studio), Some(dialog)) = (
            self.root.clone(),
            self.develop.studio(),
            self.lr_import.as_mut(),
        ) else {
            return;
        };
        dialog.stage = Stage::Importing;
        let stop = dialog.stop.clone();
        let progress = dialog.progress.clone();
        env.jobs.spawn(Lane::Long, JOB, move || {
            let report = crate::lightroom::import::run(&root, studio, &paths, &stop, &progress);
            Msg::LrImported { root, report }
        });
    }

    /// A picker answered for the dialog `id` (an earlier dialog's answer is dropped).
    pub(super) fn lr_picked(&mut self, id: u64, paths: Vec<PathBuf>) {
        if let Some(dialog) = &mut self.lr_import
            && dialog.id == id
            && dialog.stage == Stage::Choosing
        {
            dialog.add(paths);
        }
    }

    /// The import ended: the presets are read again, a sync is asked for and the dialog shows
    /// the summary (a toast says how many when it was closed meanwhile).
    pub(super) fn lr_imported(&mut self, root: PathBuf, report: ImportReport, env: &mut Env) {
        if self.root.as_deref() != Some(root.as_path()) {
            // Another catalogue's answer: its presets are not this one's, but the dialog must
            // not wait for an answer that will never come.
            if let Some(dialog) = &mut self.lr_import
                && dialog.stage == Stage::Importing
            {
                dialog.stage = Stage::Done(ImportReport::default());
            }
            return;
        }
        if !report.imported.is_empty() {
            self.develop.presets_imported();
        }
        match &mut self.lr_import {
            Some(dialog) if dialog.stage == Stage::Importing => {
                dialog.stage = Stage::Done(report);
            }
            _ => env
                .toasts
                .push(Level::Info, lr_import::imported_text(report.imported.len())),
        }
    }

    /// The import job panicked: the dialog stops waiting.
    pub(super) fn lr_import_failed(&mut self, plain: &str, env: &mut Env) {
        if let Some(dialog) = &mut self.lr_import
            && dialog.stage == Stage::Importing
        {
            self.lr_import = None;
            env.toasts.push(Level::Error, plain.to_owned());
        }
    }
}

/// Opens the file (several) or folder picker on its own thread for the dialog `id`.
fn pick(id: u64, folder: bool, env: &mut Env) {
    let reporter = env.jobs.reporter();
    let spawned = std::thread::Builder::new()
        .name("lf-picker".into())
        .spawn(move || {
            let dialog = rfd::FileDialog::new().set_title(lr_import::TITLE);
            let paths = if folder {
                dialog.pick_folder().into_iter().collect()
            } else {
                dialog
                    .add_filter("Lightroom presets", lr_import::PRESET_FILES)
                    .pick_files()
                    .unwrap_or_default()
            };
            log::info!("Lightroom presets picker answered: {paths:?}");
            reporter.send(Msg::LrPicked { dialog: id, paths });
        });
    if let Err(e) = spawned {
        log::warn!("could not open the file picker: {e}");
    }
}
