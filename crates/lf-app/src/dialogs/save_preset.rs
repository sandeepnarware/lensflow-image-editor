//! "Save as preset", as the web's `PresetsPanel.vue`: a name (up to 80 characters) and
//! which parts of the current edit the preset keeps.

use super::group_picker;
use crate::theme;
use eframe::egui::{self, RichText};
use lf_settings::groups::{DEFAULT_COPY_GROUPS, GroupKey};

pub const TITLE: &str = "Save as preset";
pub const TEXT: &str =
    "Everyone in your studio can use it. Choose which parts of this edit it keeps.";
pub const NAME: &str = "Name";
pub const HINT: &str = "e.g. Wedding warm";
pub const SAVE: &str = "Save preset";
pub const CANCEL: &str = "Cancel";
pub const TAKEN: &str = "You already have a preset with this name.";
pub const AI_MASKS: &str = "AI masks (found again on each photo)";
pub const AI_MASKS_HINT: &str = "The sky, subject or people masks keep their settings and are found again in each photo the preset is applied to";
/// The longest name the server accepts.
pub const MAX_NAME: usize = 80;

/// What the dialog asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveAction {
    None,
    Save {
        name: String,
        groups: Vec<GroupKey>,
        /// Keep the photo's AI masks as adaptive ones.
        ai_masks: bool,
    },
    Cancel,
}

#[derive(Debug, Clone)]
pub struct SavePresetDialog {
    pub name: String,
    pub groups: Vec<GroupKey>,
    /// Why the last save was refused (a name already taken).
    pub error: Option<String>,
    /// The photo has AI masks: whether the preset keeps them (`None`: it has none).
    pub ai_masks: Option<bool>,
}

impl Default for SavePresetDialog {
    fn default() -> Self {
        SavePresetDialog {
            name: String::new(),
            groups: DEFAULT_COPY_GROUPS.to_vec(),
            error: None,
            ai_masks: None,
        }
    }
}

/// The name as saved: trimmed, at most [`MAX_NAME`] characters.
pub fn clean_name(name: &str) -> String {
    name.trim().chars().take(MAX_NAME).collect()
}

impl SavePresetDialog {
    pub fn show(&mut self, ctx: &egui::Context) -> SaveAction {
        let mut action = SaveAction::None;
        let _ = egui::Modal::new(egui::Id::unique("save-preset")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.heading(TITLE);
            ui.label(RichText::new(TEXT).color(theme::MUTED));
            ui.add_space(8.0);
            let label = ui.label(NAME);
            let edit = egui::TextEdit::singleline(&mut self.name)
                .hint_text(HINT)
                .char_limit(MAX_NAME);
            let field = ui.add(edit).labelled_by(label.id);
            if field.changed() {
                self.error = None;
            }
            if let Some(e) = &self.error {
                ui.label(RichText::new(e).color(theme::DANGER));
            }
            ui.add_space(8.0);
            group_picker::show(ui, &mut self.groups, None);
            if let Some(keep) = &mut self.ai_masks {
                ui.checkbox(keep, AI_MASKS).on_hover_text(AI_MASKS_HINT);
            }
            ui.add_space(12.0);
            let name = clean_name(&self.name);
            let masks = self.ai_masks == Some(true);
            let can = !name.is_empty() && (!self.groups.is_empty() || masks);
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.horizontal(|ui| {
                if (ui.add_enabled(can, egui::Button::new(SAVE)).clicked() || (enter && can))
                    && action == SaveAction::None
                {
                    action = SaveAction::Save {
                        name: name.clone(),
                        groups: self.groups.clone(),
                        ai_masks: masks,
                    };
                }
                if ui.button(CANCEL).clicked() {
                    action = SaveAction::Cancel;
                }
            });
        });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && action == SaveAction::None {
            action = SaveAction::Cancel;
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_trimmed_and_kept_to_80_characters() {
        assert_eq!(clean_name("  Warm  "), "Warm");
        assert_eq!(clean_name(&"é".repeat(100)).chars().count(), MAX_NAME);
    }
}
