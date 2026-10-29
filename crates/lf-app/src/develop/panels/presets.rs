//! "Studio presets": this studio's presets (kept in the catalogue and synced with LensFlow).
//! A click applies one (its groups over the photo's settings); "Save preset…" keeps chosen
//! parts of the current edit; delete asks first.

use crate::theme;
use eframe::egui::{self, RichText};
use lf_catalog::presets::LocalPresetRow;

pub const TITLE: &str = "Studio presets";
pub const SAVE: &str = "Save preset…";
pub const EMPTY: &str = "Save an edit you like as a preset, and use it on any photo with one tap. \
                         Presets are shared with your team.";
pub const NO_STUDIO: &str = "Sign in to a studio to use its presets.";
pub const DELETE_TEXT: &str = "Photos already edited with it keep their look.";
pub const DELETE: &str = "Delete";
pub const DELETE_CONFIRM: &str = "Delete preset";

/// The delete question's title.
pub fn delete_title(name: &str) -> String {
    format!("Delete preset “{name}”?")
}

/// The delete button's accessible name.
pub fn delete_label(name: &str) -> String {
    format!("Delete preset {name}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresetAction {
    /// Apply the preset at this index.
    Apply(usize),
    Save,
    /// Ask to delete the preset at this index.
    Delete(usize),
}

/// A small button showing `text`, named `name` for screen readers (and as its hover text).
pub fn small_named(ui: &mut egui::Ui, text: &str, name: &str) -> egui::Response {
    let response = ui.small_button(text).on_hover_text(name);
    let name = name.to_owned();
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name.clone()));
    response
}

/// The presets; sets `hovered` to the one under the pointer.
pub fn show(
    ui: &mut egui::Ui,
    rows: &[LocalPresetRow],
    hovered: &mut Option<usize>,
) -> Option<PresetAction> {
    let mut action = None;
    if ui.button(SAVE).clicked() {
        action = Some(PresetAction::Save);
    }
    if rows.is_empty() {
        ui.label(RichText::new(EMPTY).color(theme::MUTED).small());
        return action;
    }
    for (i, row) in rows.iter().enumerate() {
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 64.0).max(40.0);
            let apply = egui::Button::new(&row.name).truncate();
            let r = ui.add_sized([width, 24.0], apply);
            if r.hovered() {
                *hovered = Some(i);
            }
            if r.clicked() {
                action = Some(PresetAction::Apply(i));
            }
            if small_named(ui, DELETE, &delete_label(&row.name)).clicked() {
                action = Some(PresetAction::Delete(i));
            }
        });
    }
    action
}
