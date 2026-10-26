//! "Apply to N photos": pasted settings, the active photo's settings or a studio preset laid
//! over each selected photo, as the web's `BatchEditDialog.vue`. The work runs on the `Io`
//! lane with its own `Catalog`: one `set_settings` and one sidecar write per photo, with a
//! stop flag checked between photos, so closing the window stops after the current photo.

use crate::clipboard::{self, Recipe};
use crate::develop::debounce::{PendingWrite, SidecarSaver};
use crate::jobs::{Jobs, Lane, Msg};
use crate::theme;
use eframe::egui::{self, RichText};
use lf_catalog::{Catalog, CatalogError, edits};
use lf_settings::EditSettings;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub const KEEP_OPEN: &str = "Keep this window open. Closing it stops after the current photo.";
pub const NOTE: &str = "Each photo keeps its own crop and spot removal.";
pub const STOP: &str = "Stop";
pub const DONE: &str = "Done";
pub const CANCEL: &str = "Cancel";
pub const WORKING: &str = "Working";
pub const DONE_ROW: &str = "Done";
pub const FAILED: &str = "Failed";
/// The source choices' labels.
pub const PASTED: &str = "Copied edits";
pub const CURRENT: &str = "This photo's edits";

/// "Apply to 3 photos".
pub fn title(n: usize) -> String {
    if n == 1 {
        "Apply to 1 photo".to_owned()
    } else {
        format!("Apply to {} photos", crate::catalog_ext::count(n))
    }
}

/// The button that starts it: "Apply to 3".
pub fn apply_label(n: usize) -> String {
    format!("Apply to {}", crate::catalog_ext::count(n))
}

/// How far a batch has got: `done` photos handled (edited or failed) of `total`; `finished`
/// once the job has ended (all done, or stopped).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BatchProgress {
    pub done: usize,
    pub total: usize,
    pub failed: Vec<(i64, String)>,
    pub finished: bool,
}

/// One photo: `partial` over its settings, recorded, and its sidecar written at once
/// (unless the sidecar beside it is damaged: that file is left alone).
pub fn apply_one(
    catalog: &Catalog,
    photo_id: i64,
    recipe: &Recipe,
    saver: &SidecarSaver,
) -> Result<(), String> {
    let text = |e: CatalogError| e.to_string();
    let photo = catalog
        .photo(photo_id)
        .map_err(text)?
        .ok_or_else(|| "This photo is no longer in the catalogue.".to_owned())?;
    let current = edits::get_settings(catalog, photo_id)
        .map_err(text)?
        .unwrap_or_else(EditSettings::default_settings);
    let next = recipe.apply(&current);
    let sidecar = edits::set_settings(catalog, photo_id, &next).map_err(text)?;
    if photo.sidecar_error.is_some() {
        log::debug!("photo {photo_id}: its sidecar is damaged; not writing");
        return Ok(());
    }
    saver.write_now(PendingWrite::new(photo_id, &photo.path, sidecar))
}

/// Applies `partial` to each of `ids` in turn, reporting after each photo; stops before the
/// next photo once `stop` is set.
pub fn run(
    catalog: &Catalog,
    ids: &[i64],
    recipe: &Recipe,
    saver: &SidecarSaver,
    stop: &AtomicBool,
    mut report: impl FnMut(&BatchProgress),
) -> BatchProgress {
    let mut p = BatchProgress {
        total: ids.len(),
        ..BatchProgress::default()
    };
    for &id in ids {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        if let Err(e) = apply_one(catalog, id, recipe, saver) {
            log::warn!("batch: photo {id} could not be edited: {e}");
            p.failed.push((id, e));
        }
        p.done += 1;
        if p.done < p.total {
            report(&p);
        }
    }
    p.finished = true;
    p
}

/// Puts the batch on the `Io` lane; progress arrives as `Msg::BatchProgress`.
pub fn spawn(
    jobs: &Jobs,
    root: PathBuf,
    ids: Vec<i64>,
    recipe: Recipe,
    saver: SidecarSaver,
    stop: Arc<AtomicBool>,
) {
    let reporter = jobs.reporter();
    jobs.spawn(Lane::Io, "batch", move || {
        let total = ids.len();
        let catalog = match Catalog::open(&root) {
            Ok(c) => c,
            Err(e) => {
                let e = e.to_string();
                return Msg::BatchProgress(BatchProgress {
                    done: total,
                    total,
                    failed: ids.iter().map(|&id| (id, e.clone())).collect(),
                    finished: true,
                });
            }
        };
        let last = run(&catalog, &ids, &recipe, &saver, &stop, |p| {
            reporter.send(Msg::BatchProgress(p.clone()));
        });
        Msg::BatchProgress(last)
    });
}

/// A row in the progress list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowState {
    Waiting,
    Working,
    Done,
    Failed(String),
}

/// Each photo's row from the progress (photos are handled in order).
pub fn row_states(ids: &[i64], p: &BatchProgress) -> Vec<RowState> {
    ids.iter()
        .enumerate()
        .map(|(i, id)| {
            if i < p.done {
                match p.failed.iter().find(|(f, _)| f == id) {
                    Some((_, e)) => RowState::Failed(e.clone()),
                    None => RowState::Done,
                }
            } else if i == p.done && !p.finished {
                RowState::Working
            } else {
                RowState::Waiting
            }
        })
        .collect()
}

/// A source the dialog offers.
#[derive(Debug, Clone)]
pub struct Choice {
    pub label: String,
    /// Always without crop, rotation or spots: each photo keeps its own.
    recipe: Recipe,
}

impl Choice {
    /// Copied edits, without their crop, rotation and spots (as the note says).
    pub fn pasted(partial: Value) -> Choice {
        Choice {
            label: PASTED.to_owned(),
            recipe: Recipe::Paste(partial).without_geometry(),
        }
    }

    /// A studio preset, applied as a look.
    pub fn preset(name: &str, settings: Value) -> Choice {
        Choice {
            label: format!("Preset: {name}"),
            recipe: Recipe::Look(settings),
        }
    }

    pub fn recipe(&self) -> &Recipe {
        &self.recipe
    }
}

/// The settings a source lays over each photo: the active photo's edits keep the look
/// groups only (not its crop and spots), as a copy does by default.
pub fn current_choice(settings: &EditSettings) -> Choice {
    Choice {
        label: CURRENT.to_owned(),
        recipe: Recipe::Paste(clipboard::picked(
            settings,
            &lf_settings::groups::DEFAULT_COPY_GROUPS,
        )),
    }
}

/// What the dialog asks of the library this frame.
#[derive(Debug, Clone, PartialEq)]
pub enum BatchAction {
    None,
    /// Start with this recipe.
    Start(Recipe),
    /// The dialog is gone.
    Closed,
}

/// The open dialog.
pub struct BatchDialog {
    pub ids: Vec<i64>,
    names: Vec<String>,
    choices: Vec<Choice>,
    chosen: usize,
    pub stop: Arc<AtomicBool>,
    progress: Option<BatchProgress>,
    /// Closed while running: goes once the job ends.
    closing: bool,
}

impl BatchDialog {
    /// For `ids` (with their file names), offering `choices` (the first is chosen).
    pub fn new(ids: Vec<i64>, names: Vec<String>, choices: Vec<Choice>) -> BatchDialog {
        BatchDialog {
            ids,
            names,
            choices,
            chosen: 0,
            stop: Arc::new(AtomicBool::new(false)),
            progress: None,
            closing: false,
        }
    }

    pub fn running(&self) -> bool {
        self.progress.as_ref().is_some_and(|p| !p.finished)
    }

    pub fn progress(&self) -> Option<&BatchProgress> {
        self.progress.as_ref()
    }

    /// The job started.
    pub fn started(&mut self) {
        self.progress = Some(BatchProgress {
            total: self.ids.len(),
            ..BatchProgress::default()
        });
    }

    /// Progress arrived; true when the dialog should now close (it was closed while
    /// running and the job has ended).
    pub fn update(&mut self, p: BatchProgress) -> bool {
        let finished = p.finished;
        self.progress = Some(p);
        finished && self.closing
    }

    /// Closing the window: stops after the current photo when running.
    fn close(&mut self) -> BatchAction {
        if self.running() {
            self.stop.store(true, Ordering::SeqCst);
            self.closing = true;
            BatchAction::None
        } else {
            BatchAction::Closed
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> BatchAction {
        let mut action = BatchAction::None;
        let _ = egui::Modal::new(egui::Id::unique("batch-dialog")).show(ctx, |ui| {
            ui.set_max_width(440.0);
            ui.heading(title(self.ids.len()));
            ui.add_space(8.0);
            match self.progress.clone() {
                None => action = self.choose_ui(ui),
                Some(p) => action = self.progress_ui(ui, &p),
            }
        });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && action == BatchAction::None {
            action = self.close();
        }
        action
    }

    fn choose_ui(&mut self, ui: &mut egui::Ui) -> BatchAction {
        let mut action = BatchAction::None;
        let label = self
            .choices
            .get(self.chosen)
            .map_or_else(String::new, |c| c.label.clone());
        egui::ComboBox::from_label("Apply")
            .selected_text(label)
            .show_ui(ui, |ui| {
                for (i, c) in self.choices.iter().enumerate() {
                    ui.selectable_value(&mut self.chosen, i, &c.label);
                }
            });
        ui.add_space(6.0);
        ui.label(RichText::new(NOTE).color(theme::MUTED));
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let can = self.choices.get(self.chosen).is_some();
            if ui
                .add_enabled(can, egui::Button::new(apply_label(self.ids.len())))
                .clicked()
                && let Some(c) = self.choices.get(self.chosen)
            {
                action = BatchAction::Start(c.recipe.clone());
            }
            if ui.button(CANCEL).clicked() {
                action = BatchAction::Closed;
            }
        });
        action
    }

    fn progress_ui(&mut self, ui: &mut egui::Ui, p: &BatchProgress) -> BatchAction {
        let mut action = BatchAction::None;
        let total = p.total.max(1);
        ui.add(egui::ProgressBar::new(p.done as f32 / total as f32));
        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .max_height(260.0)
            .show(ui, |ui| {
                for (state, name) in row_states(&self.ids, p).iter().zip(&self.names) {
                    ui.horizontal(|ui| {
                        match state {
                            RowState::Working => {
                                ui.label(RichText::new(WORKING).color(theme::BRAND));
                            }
                            RowState::Done => {
                                ui.label(RichText::new(DONE_ROW).color(theme::SUCCESS));
                            }
                            RowState::Failed(_) => {
                                ui.label(RichText::new(FAILED).color(theme::DANGER));
                            }
                            RowState::Waiting => {
                                ui.label(RichText::new("").color(theme::MUTED));
                            }
                        }
                        let colour = if *state == RowState::Waiting {
                            theme::MUTED
                        } else {
                            theme::INK
                        };
                        ui.label(RichText::new(name).color(colour));
                        if let RowState::Failed(e) = state {
                            ui.label(RichText::new(e).color(theme::DANGER).small());
                        }
                    });
                }
            });
        ui.add_space(6.0);
        if p.finished {
            let edited = p.done - p.failed.len();
            ui.label(format!("{edited} of {} edited.", p.total));
            if ui.button(DONE).clicked() {
                action = BatchAction::Closed;
            }
        } else {
            ui.label(RichText::new(KEEP_OPEN).color(theme::MUTED).small());
            if ui.button(STOP).clicked() {
                self.stop.store(true, Ordering::SeqCst);
            }
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lf_catalog::NewPhoto;
    use serde_json::json;

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
    fn every_source_keeps_each_photos_crop_and_spots() {
        let mut onto = EditSettings::default_settings();
        onto.crop.w = 0.5;
        let carrying =
            json!({"v": 2, "exposure": 1, "crop": {"x": 0, "y": 0, "w": 1, "h": 1}, "angle": 4});
        let mut photo = EditSettings::default_settings();
        photo.crop.w = 1.0;
        photo.angle = 4.0;
        for choice in [
            Choice::pasted(carrying.clone()),
            Choice::preset("Warm", carrying),
            current_choice(&photo),
        ] {
            let next = choice.recipe().apply(&onto);
            assert_eq!(
                (next.crop, next.angle),
                (onto.crop, 0.0),
                "{}",
                choice.label
            );
        }
    }

    #[test]
    fn batch_stop_halts_after_current() {
        let (_d, c, ids) = catalogue(3);
        let stop = AtomicBool::new(false);
        let saver = SidecarSaver::new();
        let mut seen = Vec::new();
        let last = run(
            &c,
            &ids,
            &Recipe::Paste(json!({"exposure": 1.5})),
            &saver,
            &stop,
            |p| {
                seen.push(p.done);
                // The window is closed while the first photo is being edited.
                stop.store(true, Ordering::SeqCst);
            },
        );
        assert_eq!(seen, [1], "one progress report, after the first photo");
        assert_eq!((last.done, last.total, last.finished), (1, 3, true));
        let exposure = |id| edits::get_settings(&c, id).unwrap().map(|s| s.exposure);
        assert_eq!(exposure(ids[0]), Some(1.5));
        assert_eq!(exposure(ids[1]), None, "never started");
        assert_eq!(exposure(ids[2]), None);
    }

    #[test]
    fn a_failed_photo_is_reported_and_the_rest_carry_on() {
        let (_d, c, mut ids) = catalogue(2);
        ids.insert(1, 999);
        let stop = AtomicBool::new(false);
        let last = run(
            &c,
            &ids,
            &Recipe::Paste(json!({"exposure": 1})),
            &SidecarSaver::new(),
            &stop,
            |_| {},
        );
        assert_eq!((last.done, last.finished), (3, true));
        assert_eq!(last.failed.len(), 1);
        assert_eq!(last.failed[0].0, 999);
        assert_eq!(
            row_states(&ids, &last),
            [
                RowState::Done,
                RowState::Failed(last.failed[0].1.clone()),
                RowState::Done
            ]
        );
        let running = BatchProgress {
            done: 1,
            total: 3,
            failed: Vec::new(),
            finished: false,
        };
        assert_eq!(
            row_states(&ids, &running),
            [RowState::Done, RowState::Working, RowState::Waiting]
        );
    }

    #[test]
    fn a_damaged_sidecar_is_never_written() {
        let (dir, c, ids) = catalogue(1);
        let photo = dir.path().join("0.jpg");
        let side = lf_catalog::sidecar::path_for(&photo);
        std::fs::write(&side, b"{ not json").unwrap();
        lf_catalog::edits::reconcile_on_open(&c, ids[0], &photo).ok();
        assert!(c.photo(ids[0]).unwrap().unwrap().sidecar_error.is_some());
        apply_one(
            &c,
            ids[0],
            &Recipe::Paste(json!({"exposure": 1})),
            &SidecarSaver::new(),
        )
        .unwrap();
        assert_eq!(std::fs::read(&side).unwrap(), b"{ not json");
        assert_eq!(
            edits::get_settings(&c, ids[0]).unwrap().unwrap().exposure,
            1.0
        );
    }
}
