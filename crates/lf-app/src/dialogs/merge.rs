//! Library → Merge to HDR (`Ctrl+H`) and Merge to panorama (`Ctrl+M`): what will happen, then
//! the merge's progress (with Stop) and how it ended. The work is [`crate::merge_job::run`] on the `Long` lane.

use crate::catalog_ext::count;
use crate::merge_job::MergeKind;
use crate::theme;
use eframe::egui::{self, Key, RichText};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub const MERGE: &str = "Merge";
pub const CANCEL: &str = "Cancel";
pub const CLOSE: &str = "Close";
pub const STOP: &str = "Stop";
pub const DEGHOST: &str = "Remove ghosts (people or branches that moved)";
pub const ROWS: &str = "Rows";
pub const FILL_EDGES: &str = "Fill edges (AI)";
pub const FILL_EDGES_HINT: &str = "Keep the whole panorama and fill its empty corners from the picture, instead of cropping them away";
pub const ABOUT_HDR: &str = "The photos are lined up and blended, each part taken from the frames where it is best exposed. The result is a 16-bit TIFF beside the first photo, catalogued with it, ready to edit.";
pub const ABOUT_PANORAMA: &str = "The photos are stitched in their order in the grid, left to right or right to left. For several rows, select them row by row, top row first. Neighbours should overlap by a third or so. The result is a 16-bit TIFF beside the first photo, catalogued with it, ready to edit.";
pub const ABOUT_FOCUS: &str = "The photos (one subject, focused at different depths) are lined up and each part is taken from the photo where it is sharp. The result is a 16-bit TIFF beside the first photo, catalogued with it, ready to edit.";

/// "Merge 3 photos to HDR", "Merge 5 photos to a panorama".
pub fn title(kind: MergeKind, n: usize) -> String {
    match kind {
        MergeKind::Hdr => format!("Merge {} photos to HDR", count(n)),
        MergeKind::Panorama => format!("Merge {} photos to a panorama", count(n)),
        MergeKind::Focus => format!("Stack {} photos for focus", count(n)),
    }
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
    pub kind: MergeKind,
    pub stage: Stage,
    pub stop: Arc<AtomicBool>,
    /// HDR: leave out what moved between the frames (on unless turned off).
    pub deghost: bool,
    /// Panorama: how many rows the photos make.
    pub rows: usize,
    /// Panorama: fill the empty edges with AI rather than crop them.
    pub fill_edges: bool,
    /// The AI tools may run (the studio in good standing).
    pub ai_allowed: bool,
}

impl MergeDialog {
    pub fn new(ids: Vec<i64>, kind: MergeKind) -> MergeDialog {
        MergeDialog {
            ids,
            kind,
            stage: Stage::Confirm,
            stop: Arc::new(AtomicBool::new(false)),
            deghost: true,
            rows: 1,
            fill_edges: false,
            ai_allowed: false,
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
            ui.heading(title(self.kind, self.ids.len()));
            ui.add_space(8.0);
            match self.stage.clone() {
                Stage::Confirm => {
                    let about = match self.kind {
                        MergeKind::Hdr => ABOUT_HDR,
                        MergeKind::Panorama => ABOUT_PANORAMA,
                        MergeKind::Focus => ABOUT_FOCUS,
                    };
                    ui.label(RichText::new(about).color(theme::MUTED));
                    if self.kind == MergeKind::Panorama {
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            let label = ui.label(ROWS);
                            ui.add(egui::Slider::new(&mut self.rows, 1..=4))
                                .labelled_by(label.id)
                                .on_hover_text("More than one: the photos come row by row, top row first");
                        });
                        ui.add_enabled(
                            self.ai_allowed,
                            egui::Checkbox::new(&mut self.fill_edges, FILL_EDGES),
                        )
                        .on_hover_text(FILL_EDGES_HINT)
                        .on_disabled_hover_text(crate::develop::tools::masks::AI_OFF);
                    }
                    if self.kind == MergeKind::Hdr {
                        ui.add_space(6.0);
                        ui.checkbox(&mut self.deghost, DEGHOST)
                            .on_hover_text("Something that moved between the shots shows once, as in the middle photo");
                    }
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

/// Whether `n` selected photos can be merged into `kind`.
pub fn can_merge(kind: MergeKind, n: usize) -> bool {
    (2..=kind.max_frames()).contains(&n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_hdr_takes_two_to_seven_photos_and_a_panorama_up_to_twelve() {
        assert!(!can_merge(MergeKind::Hdr, 1));
        assert!(can_merge(MergeKind::Hdr, 2) && can_merge(MergeKind::Hdr, 7));
        assert!(!can_merge(MergeKind::Hdr, 8));
        assert!(can_merge(MergeKind::Panorama, 24));
        let mut d = MergeDialog::new(vec![1, 2, 3], MergeKind::Hdr);
        d.started();
        assert_eq!(d.stage, Stage::Running { done: 0, total: 6 });
        assert_eq!(title(MergeKind::Hdr, 3), "Merge 3 photos to HDR");
        assert_eq!(
            title(MergeKind::Panorama, 5),
            "Merge 5 photos to a panorama"
        );
    }
}
