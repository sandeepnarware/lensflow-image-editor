//! Photo → Merge to HDR (`Ctrl+H`): what will happen, then the merge's progress (with Stop)
//! and how it ended. The work is [`crate::merge_job::run`] on the `Long` lane.

use crate::catalog_ext::count;
use crate::merge_job::MAX_FRAMES;
use crate::theme;
use eframe::egui::{self, Key, RichText};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub const MERGE: &str = "Merge";
pub const CANCEL: &str = "Cancel";
pub const CLOSE: &str = "Close";
pub const STOP: &str = "Stop";
pub const ABOUT: &str = "The photos are lined up and blended, each part taken from the frames where it is best exposed. The result is a 16-bit TIFF beside the first photo, catalogued with it, ready to edit.";

/// "Merge 3 photos to HDR".
pub fn title(n: usize) -> String {
    format!("Merge {} photos to HDR", count(n))
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stage {
    Confirm,
    Running { done: usize, total: usize },
    Done(Result<PathBuf, String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeAction {
    None,
    Start,
    Close,
}

pub struct MergeDialog {
    pub ids: Vec<i64>,
    pub stage: Stage,
    pub stop: Arc<AtomicBool>,
}

impl MergeDialog {
    pub fn new(ids: Vec<i64>) -> MergeDialog {
        MergeDialog {
            ids,
            stage: Stage::Confirm,
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn running(&self) -> bool {
        matches!(self.stage, Stage::Running { .. })
    }

    pub fn started(&mut self) {
        self.stop.store(false, Ordering::SeqCst);
        self.stage = Stage::Running {
            done: 0,
            total: self.ids.len() + 3,
        };
    }

    pub fn progress(&mut self, done: usize, total: usize) {
        if self.running() {
            self.stage = Stage::Running { done, total };
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> MergeAction {
        let mut action = MergeAction::None;
        let _ = egui::Modal::new(egui::Id::unique("merge-dialog")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.heading(title(self.ids.len()));
            ui.add_space(8.0);
            match self.stage.clone() {
                Stage::Confirm => {
                    ui.label(RichText::new(ABOUT).color(theme::MUTED));
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui.button(MERGE).clicked() {
                            action = MergeAction::Start;
                        }
                        if ui.button(CANCEL).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                            action = MergeAction::Close;
                        }
                    });
                }
                Stage::Running { done, total } => {
                    let step = if done < self.ids.len() {
                        format!("Reading photo {} of {}", done + 1, self.ids.len())
                    } else if done == self.ids.len() {
                        "Lining the photos up".to_owned()
                    } else {
                        "Blending".to_owned()
                    };
                    ui.add(egui::ProgressBar::new(done as f32 / total.max(1) as f32).text(step));
                    ui.add_space(8.0);
                    let stopping = self.stop.load(Ordering::SeqCst);
                    if ui.add_enabled(!stopping, egui::Button::new(STOP)).clicked() {
                        self.stop.store(true, Ordering::SeqCst);
                    }
                }
                Stage::Done(result) => {
                    match result {
                        Ok(path) => ui.label(format!(
                            "Merged into {}.",
                            path.file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default()
                        )),
                        Err(e) => ui.label(RichText::new(e).color(theme::WARNING)),
                    };
                    ui.add_space(12.0);
                    if ui.button(CLOSE).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                        action = MergeAction::Close;
                    }
                }
            }
        });
        action
    }
}

/// Whether `n` selected photos can be merged.
pub fn can_merge(n: usize) -> bool {
    (2..=MAX_FRAMES).contains(&n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_to_seven_photos_can_be_merged() {
        assert!(!can_merge(1));
        assert!(can_merge(2) && can_merge(7));
        assert!(!can_merge(8));
        let mut d = MergeDialog::new(vec![1, 2, 3]);
        d.started();
        assert_eq!(d.stage, Stage::Running { done: 0, total: 6 });
        assert_eq!(title(3), "Merge 3 photos to HDR");
    }
}
