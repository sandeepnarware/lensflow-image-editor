//! "Studio presets": this studio's presets (kept in the catalogue and synced with LensFlow).
//! A click applies one (its groups over the photo's settings); "Save preset…" keeps chosen
//! parts of the current edit; delete asks first; "Import from Lightroom…" opens the import.
//! Names are drawn through [`shown_name`]: an imported one may hold characters the fonts lack.

use crate::lightroom::import::shown_name;
use crate::theme;
use eframe::egui::{self, RichText};
use lf_catalog::presets::LocalPresetRow;

pub const TITLE: &str = "Studio presets";
pub const SAVE: &str = "Save preset…";
pub const IMPORT_LR: &str = "Import from Lightroom…";
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
    /// Open "Import Lightroom presets…".
    ImportLr,
    /// Ask to delete the preset at this index.
    Delete(usize),
}

/// A small button showing `text`, named `name` for screen readers (and as its hover text).
/// The screen reader gets `name` whole; the painted hover text goes through [`shown_name`].
pub fn small_named(ui: &mut egui::Ui, text: &str, name: &str) -> egui::Response {
    let hover = shown_name(ui.ctx(), name);
    let response = ui.small_button(text).on_hover_text(hover);
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
    }
    for (i, row) in rows.iter().enumerate() {
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 64.0).max(40.0);
            let name = shown_name(ui.ctx(), &row.name);
            let apply = egui::Button::new(&name).truncate();
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
    if ui.link(IMPORT_LR).clicked() {
        action = Some(PresetAction::ImportLr);
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable as _;

    #[test]
    fn a_name_the_fonts_lack_is_drawn_with_question_marks_but_read_out_whole() {
        let real = "Studio - \u{5915}\u{713c}\u{3051}";
        let rows = vec![LocalPresetRow {
            id: 1,
            studio_id: 7,
            name: real.to_owned(),
            settings: serde_json::json!({"v": 2}),
            updated_at: "2026-11-01T00:00:00Z".to_owned(),
            remote_id: None,
            synced_at: None,
            deleted_at: None,
        }];
        let mut h = Harness::builder().with_size([400.0, 300.0]).build_ui_state(
            |ui, rows: &mut Vec<LocalPresetRow>| {
                show(ui, rows, &mut None);
            },
            rows,
        );
        h.step();
        assert!(
            h.query_by_label("Studio - ???").is_some(),
            "the painted name"
        );
        // The screen reader hears the real name; hovering paints its tooltip safely.
        h.get_by_label(&delete_label(real)).hover();
        h.run_steps(30);
    }
}
