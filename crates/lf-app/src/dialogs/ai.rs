//! The AI tools' dialog (AI Denoise, AI Upscale, Retouch portrait, Remove background): the
//! tool's one setting, what the first use downloads, then the progress (downloading, then
//! photo by photo, with Stop) and what was made. The work is [`crate::ai_job::run`] on the
//! `Long` lane.

use crate::ai_job::{AiTool, Report, Step};
use crate::catalog_ext::count;
use crate::theme;
use eframe::egui::{self, Key, RichText};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub const START: &str = "Start";
pub const CANCEL: &str = "Cancel";
pub const CLOSE: &str = "Close";
pub const STOP: &str = "Stop";
pub const STRENGTH: &str = "Strength";
pub const AMOUNT: &str = "Smoothing";
pub const ON_THIS_COMPUTER: &str = "Runs on this computer: nothing is uploaded.";
pub const ABOUT_DENOISE: &str = "Removes noise while keeping detail. Each photo is rendered with its edits and saved beside it as a new 16-bit TIFF; the original is not changed.";
pub const ABOUT_UPSCALE: &str = "Enlarges each photo with detail filled in, saved beside it as a new 16-bit TIFF. Very large results are capped at 120 megapixels.";
pub const ABOUT_RETOUCH: &str = "Finds the faces and smooths their skin, leaving eyes, lips and hair sharp. Saved beside each photo as a new 16-bit TIFF.";
pub const ABOUT_CUTOUT: &str = "Keeps the main subject and makes the background transparent, saved beside each photo as a PNG.";

/// "AI Denoise: 3 photos".
pub fn title(tool: AiTool, n: usize) -> String {
    if n == 1 {
        format!("{}: 1 photo", tool.name())
    } else {
        format!("{}: {} photos", tool.name(), count(n))
    }
}

/// "The first time, this downloads 78 MB (the AI engine and its model) and keeps it."
pub fn download_text(bytes: u64) -> Option<String> {
    (bytes > 0).then(|| {
        format!(
            "The first time, this downloads {} MB (the AI engine and its model) and keeps it for next time.",
            bytes.div_ceil(1_000_000)
        )
    })
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stage {
    Confirm,
    Running(Step),
    Done(Result<Report, String>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AiAction {
    None,
    Start(AiTool),
    Close,
}

pub struct AiDialog {
    pub ids: Vec<i64>,
    pub tool: AiTool,
    pub stage: Stage,
    pub stop: Arc<AtomicBool>,
    /// What the first run still has to download (bytes).
    pub download: u64,
}

impl AiDialog {
    pub fn new(ids: Vec<i64>, tool: AiTool, download: u64) -> AiDialog {
        AiDialog {
            ids,
            tool,
            stage: Stage::Confirm,
            stop: Arc::new(AtomicBool::new(false)),
            download,
        }
    }

    pub fn running(&self) -> bool {
        matches!(self.stage, Stage::Running(_))
    }

    pub fn started(&mut self) {
        self.stop.store(false, Ordering::SeqCst);
        self.stage = Stage::Running(Step::Download { done: 0, total: 0 });
    }

    pub fn progress(&mut self, step: Step) {
        if self.running() {
            self.stage = Stage::Running(step);
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> AiAction {
        let mut action = AiAction::None;
        let _ = egui::Modal::new(egui::Id::unique("ai-dialog")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.heading(title(self.tool, self.ids.len()));
            ui.add_space(8.0);
            match self.stage.clone() {
                Stage::Confirm => action = self.confirm_ui(ui),
                Stage::Running(step) => self.running_ui(ui, step),
                Stage::Done(result) => {
                    done_ui(ui, &result);
                    ui.add_space(12.0);
                    if ui.button(CLOSE).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                        action = AiAction::Close;
                    }
                }
            }
        });
        action
    }

    fn confirm_ui(&mut self, ui: &mut egui::Ui) -> AiAction {
        let about = match self.tool {
            AiTool::Denoise { .. } => ABOUT_DENOISE,
            AiTool::Upscale { .. } => ABOUT_UPSCALE,
            AiTool::Retouch { .. } => ABOUT_RETOUCH,
            AiTool::Cutout => ABOUT_CUTOUT,
        };
        ui.label(RichText::new(about).color(theme::MUTED));
        ui.add_space(8.0);
        match &mut self.tool {
            AiTool::Denoise { strength } => {
                let mut pct = *strength * 100.0;
                let label = ui.label(STRENGTH);
                ui.add(egui::Slider::new(&mut pct, 10.0..=100.0).suffix(" %"))
                    .labelled_by(label.id);
                *strength = pct / 100.0;
            }
            AiTool::Upscale { factor } => {
                ui.horizontal(|ui| {
                    ui.radio_value(factor, 2, "2× (twice as wide)");
                    ui.radio_value(factor, 4, "4×");
                });
            }
            AiTool::Retouch { amount } => {
                let mut pct = *amount * 100.0;
                let label = ui.label(AMOUNT);
                ui.add(egui::Slider::new(&mut pct, 10.0..=100.0).suffix(" %"))
                    .labelled_by(label.id);
                *amount = pct / 100.0;
            }
            AiTool::Cutout => {}
        }
        ui.add_space(8.0);
        if let Some(text) = download_text(self.download) {
            ui.label(RichText::new(text).color(theme::WARNING));
        }
        ui.label(RichText::new(ON_THIS_COMPUTER).color(theme::MUTED).small());
        ui.add_space(12.0);
        let mut action = AiAction::None;
        ui.horizontal(|ui| {
            if ui.button(START).clicked() {
                action = AiAction::Start(self.tool);
            }
            if ui.button(CANCEL).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                action = AiAction::Close;
            }
        });
        action
    }

    fn running_ui(&mut self, ui: &mut egui::Ui, step: Step) {
        let (fraction, text) = match step {
            Step::Download { done, total } => (
                if total == 0 {
                    0.0
                } else {
                    done as f32 / total as f32
                },
                format!(
                    "Downloading the AI model: {} of {} MB",
                    done / 1_000_000,
                    total.div_ceil(1_000_000)
                ),
            ),
            Step::Photo {
                index,
                count: n,
                done,
                total,
            } => (
                (index as f32 + done as f32 / total.max(1) as f32) / n.max(1) as f32,
                format!("Photo {} of {}", index + 1, count(n)),
            ),
        };
        ui.add(egui::ProgressBar::new(fraction).text(text));
        ui.add_space(8.0);
        let stopping = self.stop.load(Ordering::SeqCst);
        if ui.add_enabled(!stopping, egui::Button::new(STOP)).clicked() {
            self.stop.store(true, Ordering::SeqCst);
        }
    }
}

fn done_ui(ui: &mut egui::Ui, result: &Result<Report, String>) {
    match result {
        Ok(report) => {
            let n = report.written.len();
            ui.label(match n {
                0 => "No new photo was made.".to_owned(),
                1 => format!(
                    "Made {}.",
                    report.written[0]
                        .file_name()
                        .map(|f| f.to_string_lossy().into_owned())
                        .unwrap_or_default()
                ),
                n => format!("Made {} new photos beside the originals.", count(n)),
            });
            for (name, why) in &report.failed {
                ui.label(RichText::new(format!("{name}: {why}")).color(theme::WARNING));
            }
        }
        Err(e) => {
            ui.label(RichText::new(e).color(theme::WARNING));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_and_download_note_read_well() {
        assert_eq!(
            title(AiTool::Denoise { strength: 0.7 }, 1),
            "AI Denoise: 1 photo"
        );
        assert_eq!(title(AiTool::Cutout, 3), "Remove background: 3 photos");
        assert_eq!(download_text(0), None);
        assert_eq!(
            download_text(77_500_000).unwrap(),
            "The first time, this downloads 78 MB (the AI engine and its model) and keeps it for next time."
        );
    }

    #[test]
    fn progress_shows_only_while_running() {
        let mut d = AiDialog::new(vec![1, 2], AiTool::Cutout, 0);
        d.progress(Step::Download { done: 1, total: 2 });
        assert_eq!(d.stage, Stage::Confirm);
        d.started();
        d.progress(Step::Photo {
            index: 1,
            count: 2,
            done: 0,
            total: 1,
        });
        assert!(d.running());
    }
}
