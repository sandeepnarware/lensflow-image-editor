//! The built-in looks: a 4-column grid of the 12 `LOOKS`. A click applies the look
//! (`apply_look`: it replaces the look groups and keeps the crop and spots); which look was
//! applied last is not tracked, as on the web.

use eframe::egui;
use lf_settings::looks::LOOKS;

pub const TITLE: &str = "Looks";
pub const COLUMNS: usize = 4;

/// The look clicked this frame (its index in `LOOKS`).
/// The looks; returns the one clicked, and sets `hovered` to the one under the pointer.
pub fn show(ui: &mut egui::Ui, hovered: &mut Option<usize>) -> Option<usize> {
    let mut clicked = None;
    let gap = ui.spacing().item_spacing.x;
    let width = ((ui.available_width() - gap * (COLUMNS as f32 - 1.0)) / COLUMNS as f32).max(20.0);
    egui::Grid::new("looks-grid")
        .num_columns(COLUMNS)
        .spacing([gap, gap])
        .show(ui, |ui| {
            for (i, look) in LOOKS.iter().enumerate() {
                let button = egui::Button::new(egui::RichText::new(look.name).small())
                    .truncate()
                    .min_size(egui::vec2(width, 28.0));
                let r = ui.add_sized([width, 28.0], button).on_hover_text(look.name);
                if r.hovered() {
                    *hovered = Some(i);
                }
                if r.clicked() {
                    clicked = Some(i);
                }
                if i % COLUMNS == COLUMNS - 1 {
                    ui.end_row();
                }
            }
        });
    clicked
}
