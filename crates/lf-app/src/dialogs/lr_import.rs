//! Develop → Import Lightroom presets…: choose preset files, a folder or a `.zip` pack (or
//! the folders Lightroom and Camera Raw keep their presets in), import them, then a summary:
//! how many were imported, each preset's notes on what was left out or approximated, the
//! files that failed and why, and any left out at the studio's limit. The work is
//! [`crate::lightroom::import`] on the `Long` lane; the pickers run on their own threads.

use crate::catalog_ext::count;
use crate::lightroom::import::{ImportReport, MAX_PRESETS, Progress, shown_name};
use crate::theme;
use eframe::egui::{self, Key, RichText};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub const TITLE: &str = "Import Lightroom presets";
pub const ABOUT: &str = "Presets from Lightroom Classic or Camera Raw (.xmp or .lrtemplate files, or a .zip of them) become studio presets, shared with your team. The files themselves are only read.";
pub const CHOOSE_FILES: &str = "Choose files…";
pub const CHOOSE_FOLDER: &str = "Choose folder…";
pub const FIND_ADOBE: &str = "Find Lightroom's presets";
pub const NO_ADOBE: &str = "Lightroom's preset folders were not found on this computer.";
pub const NOTHING_CHOSEN: &str = "Nothing chosen yet.";
pub const CLEAR: &str = "Clear";
pub const IMPORT: &str = "Import";
pub const CANCEL: &str = "Cancel";
pub const IMPORTING: &str = "Importing presets…";
pub const STOP: &str = "Stop";
pub const CLOSE: &str = "Close";
pub const FAILED_TITLE: &str = "Could not be imported";
pub const NOTES_TITLE: &str = "Imported with notes";
pub const MATCH_EXAMPLE: &str = "Match a Lightroom example…";
pub const IMPORTED_TITLE: &str = "Imported presets";
/// Shown when the import's answer was for another catalogue than the one open now.
pub const OTHER_CATALOGUE: &str =
    "The presets were imported into another catalogue, not the one that is open now.";

/// The files the "Choose files…" picker offers.
pub const PRESET_FILES: &[&str] = &["xmp", "lrtemplate", "zip"];

/// "3 presets imported." (or "1 preset imported.").
pub fn imported_text(n: usize) -> String {
    if n == 1 {
        "1 preset imported.".to_owned()
    } else {
        format!("{} presets imported.", count(n))
    }
}

/// The profiles line: Lightroom profiles are looks of another kind, left alone on purpose.
pub fn skipped_profiles_text(n: usize) -> String {
    let what = if n == 1 { "profile" } else { "profiles" };
    format!(
        "Skipped: {} {what} (Lightroom profiles are not presets).",
        count(n)
    )
}

/// The left-out line: the studio is full.
pub fn left_out_text(n: usize) -> String {
    format!(
        "{} left out: a studio keeps at most {} presets.",
        count(n),
        count(MAX_PRESETS)
    )
}

/// "2 of 5 files" while importing.
pub fn progress_text(done: usize, total: usize) -> String {
    format!("{} of {} files", count(done), count(total))
}

/// The summary's line after Stop.
pub fn stopped_text(not_processed: usize) -> String {
    if not_processed == 1 {
        "The import was stopped: 1 file was not processed.".to_owned()
    } else {
        format!(
            "The import was stopped: {} files were not processed.",
            count(not_processed)
        )
    }
}

static IDS: AtomicU64 = AtomicU64::new(1);

/// Where the dialog is.
#[derive(Debug, Clone, PartialEq)]
pub enum Stage {
    Choosing,
    Importing,
    Done(ImportReport),
    /// The answer was for another catalogue ([`OTHER_CATALOGUE`]).
    Elsewhere,
}

/// What the dialog asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LrImportAction {
    None,
    /// Open the file picker (several files).
    ChooseFiles,
    /// Open the folder picker.
    ChooseFolder,
    /// Import what was chosen.
    Import(Vec<PathBuf>),
    /// Open "Match a Lightroom example…" for the imported preset of this name.
    MatchExample(String),
    Close,
}

pub struct LrImportDialog {
    /// This dialog's own id: a picker's answer for an earlier dialog is dropped.
    pub id: u64,
    /// The files and folders chosen so far.
    pub chosen: Vec<PathBuf>,
    /// Adobe's preset folders on this computer (none on Linux).
    pub adobe: Vec<PathBuf>,
    pub stage: Stage,
    pub stop: Arc<AtomicBool>,
    /// Files done of the files gathered, written by the job.
    pub progress: Arc<Progress>,
}

impl LrImportDialog {
    pub fn new(adobe: Vec<PathBuf>) -> LrImportDialog {
        LrImportDialog {
            id: IDS.fetch_add(1, Ordering::Relaxed),
            chosen: Vec::new(),
            adobe,
            stage: Stage::Choosing,
            stop: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(Progress::default()),
        }
    }

    /// Adds `paths` to what is chosen (each once).
    pub fn add(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        for p in paths {
            if !self.chosen.contains(&p) {
                self.chosen.push(p);
            }
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> LrImportAction {
        let mut action = LrImportAction::None;
        let _ = egui::Modal::new(egui::Id::unique("lr-import")).show(ctx, |ui| {
            ui.set_width(480.0);
            ui.heading(TITLE);
            ui.add_space(8.0);
            match &self.stage {
                Stage::Choosing => action = self.choosing_ui(ui),
                Stage::Importing => {
                    let done = self.progress.done.load(Ordering::Relaxed);
                    let total = self.progress.total.load(Ordering::Relaxed);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        // Until the files are gathered there is no count yet.
                        if total == 0 {
                            ui.label(IMPORTING);
                        } else {
                            ui.label(progress_text(done, total));
                        }
                    });
                    if total > 0 {
                        ui.add(egui::ProgressBar::new(done as f32 / total as f32));
                    }
                    // The job counts on its own thread: look again shortly.
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(200));
                    ui.add_space(8.0);
                    let stopping = self.stop.load(Ordering::SeqCst);
                    if ui.add_enabled(!stopping, egui::Button::new(STOP)).clicked() {
                        self.stop.store(true, Ordering::SeqCst);
                    }
                }
                Stage::Done(report) => {
                    if let Some(name) = summary_ui(ui, report) {
                        action = LrImportAction::MatchExample(name);
                    }
                    ui.add_space(12.0);
                    if ui.button(CLOSE).clicked() {
                        action = LrImportAction::Close;
                    }
                }
                Stage::Elsewhere => {
                    ui.label(OTHER_CATALOGUE);
                    ui.add_space(12.0);
                    if ui.button(CLOSE).clicked() {
                        action = LrImportAction::Close;
                    }
                }
            }
        });
        let running = self.stage == Stage::Importing;
        if !running && action == LrImportAction::None && ctx.input(|i| i.key_pressed(Key::Escape)) {
            action = LrImportAction::Close;
        }
        action
    }

    fn choosing_ui(&mut self, ui: &mut egui::Ui) -> LrImportAction {
        let mut action = LrImportAction::None;
        ui.label(RichText::new(ABOUT).color(theme::MUTED));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button(CHOOSE_FILES).clicked() {
                action = LrImportAction::ChooseFiles;
            }
            if ui.button(CHOOSE_FOLDER).clicked() {
                action = LrImportAction::ChooseFolder;
            }
            let find = ui
                .add_enabled(!self.adobe.is_empty(), egui::Button::new(FIND_ADOBE))
                .on_disabled_hover_text(NO_ADOBE);
            if find.clicked() {
                let adobe = self.adobe.clone();
                self.add(adobe);
            }
        });
        ui.add_space(8.0);
        if self.chosen.is_empty() {
            ui.label(RichText::new(NOTHING_CHOSEN).color(theme::MUTED).small());
        } else {
            egui::ScrollArea::vertical()
                .id_salt("lr-import-chosen")
                .max_height(120.0)
                .show(ui, |ui| {
                    for p in &self.chosen {
                        let text = shown_name(ui.ctx(), &p.display().to_string());
                        ui.add(egui::Label::new(RichText::new(text).small()).truncate());
                    }
                });
            if ui.small_button(CLEAR).clicked() {
                self.chosen.clear();
            }
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let can = !self.chosen.is_empty();
            if ui.add_enabled(can, egui::Button::new(IMPORT)).clicked() {
                action = LrImportAction::Import(self.chosen.clone());
            }
            if ui.button(CANCEL).clicked() {
                action = LrImportAction::Close;
            }
        });
        action
    }
}

/// The summary: imported (each with "Match a Lightroom example…", whose preset's name is
/// answered), with notes (each preset's opens to show them), failed, skipped profiles, left out.
fn summary_ui(ui: &mut egui::Ui, report: &ImportReport) -> Option<String> {
    let mut matched = None;
    ui.label(RichText::new(imported_text(report.imported.len())).strong());
    if let Some(n) = report.stopped {
        ui.label(RichText::new(stopped_text(n)).color(theme::WARNING));
    }
    if report.skipped_profiles > 0 {
        ui.label(RichText::new(skipped_profiles_text(report.skipped_profiles)).color(theme::MUTED));
    }
    let noted: Vec<_> = report
        .imported
        .iter()
        .filter(|(_, notes)| !notes.is_empty())
        .collect();
    egui::ScrollArea::vertical()
        .id_salt("lr-import-summary")
        .max_height(360.0)
        .show(ui, |ui| {
            if !report.imported.is_empty() {
                ui.add_space(6.0);
                ui.label(IMPORTED_TITLE);
                for (name, _) in &report.imported {
                    ui.horizontal(|ui| {
                        ui.label(shown_name(ui.ctx(), name));
                        if ui.small_button(MATCH_EXAMPLE).clicked() {
                            matched = Some(name.clone());
                        }
                    });
                }
            }
            if !noted.is_empty() {
                ui.add_space(6.0);
                ui.label(format!("{NOTES_TITLE} ({})", count(noted.len())));
                for (i, (name, notes)) in noted.iter().enumerate() {
                    egui::CollapsingHeader::new(shown_name(ui.ctx(), name))
                        .id_salt(("lr-import-notes", i))
                        .show(ui, |ui| {
                            for note in notes {
                                ui.label(RichText::new(shown_name(ui.ctx(), note)).small());
                            }
                        });
                }
            }
            if !report.failed.is_empty() {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!("{FAILED_TITLE} ({})", count(report.failed.len())))
                        .color(theme::DANGER),
                );
                for (source, reason) in &report.failed {
                    ui.label(
                        RichText::new(format!(
                            "{}: {}",
                            shown_name(ui.ctx(), source),
                            shown_name(ui.ctx(), reason)
                        ))
                        .small(),
                    );
                }
            }
            if !report.left_out.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new(left_out_text(report.left_out.len())).color(theme::WARNING));
                for name in &report.left_out {
                    ui.label(RichText::new(shown_name(ui.ctx(), name)).small());
                }
            }
        });
    matched
}
