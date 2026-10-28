//! Home → Auto-stack…: how far apart a burst's frames may be and whether duplicates anywhere
//! in the folder are stacked too, then the run's progress with Stop. The work is
//! [`crate::stack_job::run`] on the `Long` lane; the result is a toast.

use crate::theme;
use eframe::egui::{self, Key, RichText};
use lf_catalog::stacks::Grouping;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub const TITLE: &str = "Auto-stack";
pub const ABOUT: &str = "Gathers bursts (photos taken moments apart that look alike) into stacks, the sharpest on top when the photos have been through Assisted culling. Photos already in a stack stay where they are.";
pub const GAP: &str = "Longest gap within a burst";
pub const DUPLICATES: &str = "Also stack duplicates anywhere in the folder";
pub const START: &str = "Stack";
pub const CANCEL: &str = "Cancel";
pub const STOP: &str = "Stop";

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AutoStackAction {
    None,
    Start(Grouping),
    Close,
}

pub struct AutoStackDialog {
    pub ids: Vec<i64>,
    pub how: Grouping,
    /// Fingerprinting progress (done, total) once started.
    pub running: Option<(usize, usize)>,
    pub stop: Arc<AtomicBool>,
}

impl AutoStackDialog {
    pub fn new(ids: Vec<i64>) -> AutoStackDialog {
        AutoStackDialog {
            ids,
            how: Grouping::default(),
            running: None,
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> AutoStackAction {
        let mut action = AutoStackAction::None;
        let _ = egui::Modal::new(egui::Id::unique("auto-stack")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.heading(format!(
                "{TITLE}: {} photos",
                crate::catalog_ext::count(self.ids.len())
            ));
            ui.add_space(8.0);
            if let Some((done, total)) = self.running {
                let fraction = if total == 0 {
                    1.0
                } else {
                    done as f32 / total as f32
                };
                ui.add(egui::ProgressBar::new(fraction).text(format!(
                    "Reading photos: {} of {}",
                    crate::catalog_ext::count(done),
                    crate::catalog_ext::count(total)
                )));
                ui.add_space(8.0);
                let stopping = self.stop.load(Ordering::SeqCst);
                if ui.add_enabled(!stopping, egui::Button::new(STOP)).clicked() {
                    self.stop.store(true, Ordering::SeqCst);
                }
                return;
            }
            ui.label(RichText::new(ABOUT).color(theme::MUTED));
            ui.add_space(8.0);
            let label = ui.label(GAP);
            ui.add(
                egui::Slider::new(&mut self.how.burst_gap, 0.0..=10.0)
                    .step_by(0.5)
                    .suffix(" s"),
            )
            .labelled_by(label.id);
            ui.checkbox(&mut self.how.duplicates, DUPLICATES);
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button(START).clicked() {
                    action = AutoStackAction::Start(self.how);
                }
                if ui.button(CANCEL).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                    action = AutoStackAction::Close;
                }
            });
        });
        action
    }
}
