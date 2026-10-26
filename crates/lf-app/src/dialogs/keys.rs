//! Help → Keyboard shortcuts: the keyboard map, as this computer writes its keys.

use crate::shortcuts::MAP;
use eframe::egui::{self, RichText};

pub const TITLE: &str = "Keyboard shortcuts";
/// Under the list.
pub const NOTE: &str = "Keys do nothing while you type in a field or a dialog is open. The tool keys work in Develop. Hold \\ to see the photo as it was.";

/// Draws the list; true when it was closed (Close or Esc).
pub fn show(ctx: &egui::Context) -> bool {
    let mut close = false;
    let _ = egui::Modal::new(egui::Id::unique("keyboard-shortcuts")).show(ctx, |ui| {
        ui.set_width(420.0);
        ui.heading(TITLE);
        ui.add_space(8.0);
        egui::Grid::new("shortcut-list")
            .num_columns(2)
            .spacing([24.0, 6.0])
            .show(ui, |ui| {
                for (action, shortcut) in MAP.entries() {
                    ui.label(RichText::new(ctx.format_shortcut(shortcut)).monospace());
                    ui.label(action.label());
                    ui.end_row();
                }
            });
        ui.add_space(8.0);
        ui.label(RichText::new(NOTE).small());
        ui.add_space(12.0);
        if ui.button("Close").clicked() {
            close = true;
        }
    });
    close || ctx.input(|i| i.key_pressed(egui::Key::Escape))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shortcuts::Action;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable as _;

    #[test]
    fn the_list_shows_every_entry() {
        let mut h = Harness::builder()
            .with_size([600.0, 900.0])
            .build_ui_state(|ui, closed: &mut bool| *closed |= show(ui.ctx()), false);
        h.run();
        for (action, _) in MAP.entries() {
            h.get_all_by_label(action.label())
                .next()
                .unwrap_or_else(|| panic!("{action:?} is not listed"));
        }
        h.get_by_label(Action::Export.label());
        h.get_by_label("Ctrl+Shift+E");
        h.get_by_label("F1");
    }
}
