//! Modal dialogs: the group picker (copy settings), saving a preset, applying edits to many
//! photos, export, match a reference, and the window's own (About, Preferences, Keyboard
//! shortcuts).

pub mod about;
pub mod ai;
pub mod album;
pub mod batch;
pub mod export;
pub mod export_presets;
pub mod gallery_export;
pub mod group_picker;
pub mod ingest;
pub mod keys;
pub mod match_reference;
pub mod merge;
pub mod preferences;
pub mod print;
pub mod quit;
pub mod save_preset;

use crate::theme;
use eframe::egui::{self, RichText};

/// A dialog was open last frame: the window's own shortcuts wait until it closes.
pub fn modal_open(ctx: &egui::Context) -> bool {
    ctx.memory(|m| m.top_modal_layer().is_some())
}

/// A yes-or-no question: `Some(true)` when confirmed, `Some(false)` when cancelled (also
/// Esc), `None` while it waits.
pub fn confirm(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    text: &str,
    yes: &str,
    danger: bool,
) -> Option<bool> {
    let mut answer = None;
    let _ = egui::Modal::new(egui::Id::unique(id)).show(ctx, |ui| {
        ui.set_max_width(420.0);
        ui.heading(title);
        ui.add_space(8.0);
        ui.label(text);
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let mut button = egui::Button::new(if danger {
                RichText::new(yes).color(theme::BRAND_CONTRAST)
            } else {
                RichText::new(yes)
            });
            if danger {
                button = button.fill(theme::DANGER);
            }
            if ui.add(button).clicked() {
                answer = Some(true);
            }
            if ui.button("Cancel").clicked() {
                answer = Some(false);
            }
        });
    });
    if answer.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        answer = Some(false);
    }
    answer
}
