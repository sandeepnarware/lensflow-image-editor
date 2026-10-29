//! "Rename…" for the selected photos ([`lf_catalog::rename`]): a pattern with the export's
//! tokens, a preview of the first new names (worked out from the rows in memory), and Rename.
//! The catalogue checks every name again when it renames, and refuses the whole batch when
//! one is taken; the dialog then shows why and stays open.

use crate::catalog_ext::count;
use crate::theme;
use eframe::egui::{self, Key, RichText};
use lf_catalog::PhotoRow;

pub const PATTERN: &str = "New names";
pub const HINT: &str = "Tokens: {name} (the name now), {seq} (1, 2, 3…), {date} (taken), {camera}. The extension stays.";
pub const RENAME: &str = "Rename";
pub const CANCEL: &str = "Cancel";
pub const SAFE: &str =
    "Only the names change; each photo keeps its content, its folder and its edits.";
const PREVIEW: usize = 8;

pub enum RenameAction {
    None,
    Close,
    /// Rename the photos with this pattern.
    Rename(String),
}

pub struct RenameDialog {
    pub ids: Vec<i64>,
    pub pattern: String,
    pub running: bool,
    pub error: Option<String>,
}

impl RenameDialog {
    pub fn new(ids: Vec<i64>) -> RenameDialog {
        RenameDialog {
            ids,
            pattern: "{name}".to_owned(),
            running: false,
            error: None,
        }
    }

    /// `rows`: the selected photos' rows, in order.
    pub fn show(&mut self, ctx: &egui::Context, rows: &[&PhotoRow]) -> RenameAction {
        let mut action = RenameAction::None;
        let n = self.ids.len();
        egui::Modal::new(egui::Id::unique("rename-photos")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.heading(if n == 1 {
                "Rename 1 photo".to_owned()
            } else {
                format!("Rename {} photos", count(n))
            });
            ui.add_space(8.0);
            ui.label(PATTERN);
            let edit =
                ui.add(egui::TextEdit::singleline(&mut self.pattern).desired_width(f32::INFINITY));
            edit.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, PATTERN));
            if edit.changed() {
                self.error = None;
            }
            ui.label(RichText::new(HINT).color(theme::MUTED).small());
            ui.add_space(8.0);
            for (i, row) in rows.iter().take(PREVIEW).enumerate() {
                let to = lf_catalog::rename::new_name(row, i + 1, rows.len(), &self.pattern);
                let name = |p: &std::path::Path| {
                    p.file_name()
                        .map(|f| f.to_string_lossy().into_owned())
                        .unwrap_or_default()
                };
                ui.label(
                    RichText::new(format!("{}  ->  {}", name(&row.path), name(&to))).monospace(),
                );
            }
            if rows.len() > PREVIEW {
                ui.label(
                    RichText::new(format!("and {} more", count(rows.len() - PREVIEW)))
                        .color(theme::MUTED),
                );
            }
            ui.add_space(6.0);
            ui.label(RichText::new(SAFE).color(theme::MUTED).small());
            if let Some(e) = &self.error {
                ui.label(RichText::new(e).color(theme::WARNING));
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let ready = !self.running && !self.pattern.trim().is_empty();
                if ui.add_enabled(ready, egui::Button::new(RENAME)).clicked() {
                    action = RenameAction::Rename(self.pattern.clone());
                }
                if ui
                    .add_enabled(!self.running, egui::Button::new(CANCEL))
                    .clicked()
                    || (!self.running && ui.input(|i| i.key_pressed(Key::Escape)))
                {
                    action = RenameAction::Close;
                }
            });
        });
        action
    }
}
