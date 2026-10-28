//! Asking before the window closes while an export or "Apply to N photos" runs: quitting
//! stops it after the current photo.

use eframe::egui::{self, RichText};

/// The question while an export runs.
pub const EXPORT_RUNNING: &str = "An export is running. Quit anyway?";
/// The question while photos are being uploaded to a gallery.
pub const GALLERY_RUNNING: &str = "Photos are still being uploaded to a gallery. Quit anyway?";
/// The question while photos are being copied from a card.
pub const INGEST_RUNNING: &str = "Photos are still being copied from a card. Quit anyway?";
/// The question while "Apply to N photos" runs.
pub const BATCH_RUNNING: &str = "Photos are still being updated. Quit anyway?";
/// What quitting does to it.
pub const STOPS: &str = "It stops after the current photo.";
pub const QUIT: &str = "Quit";
pub const KEEP_WORKING: &str = "Keep working";

/// Shows `question`: `Some(true)` to quit, `Some(false)` to keep working (also Esc), `None`
/// while it waits.
pub fn show(ctx: &egui::Context, question: &str) -> Option<bool> {
    let mut answer = None;
    let _ = egui::Modal::new(egui::Id::unique("quit-while-working")).show(ctx, |ui| {
        ui.set_max_width(420.0);
        ui.heading(question);
        ui.add_space(8.0);
        ui.label(RichText::new(STOPS).color(crate::theme::MUTED));
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui.button(QUIT).clicked() {
                answer = Some(true);
            }
            if ui.button(KEEP_WORKING).clicked() {
                answer = Some(false);
            }
        });
    });
    if answer.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        answer = Some(false);
    }
    answer
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable as _;

    fn harness() -> Harness<'static, Option<bool>> {
        Harness::builder().with_size([800.0, 600.0]).build_ui_state(
            |ui, answer: &mut Option<bool>| {
                if let Some(a) = show(ui.ctx(), EXPORT_RUNNING) {
                    *answer = Some(a);
                }
            },
            None,
        )
    }

    #[test]
    fn the_question_offers_quit_and_keep_working() {
        let mut h = harness();
        h.step();
        h.get_by_label(EXPORT_RUNNING);
        h.get_by_label(STOPS);
        h.get_by_label(KEEP_WORKING).click();
        h.step();
        assert_eq!(*h.state(), Some(false));

        let mut h = harness();
        h.step();
        h.get_by_label(QUIT).click();
        h.step();
        assert_eq!(*h.state(), Some(true));
    }
}
