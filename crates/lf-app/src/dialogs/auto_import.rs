//! Home → Auto import…: the watched folder, the catalogued folder photos go into, and Start or
//! Stop. The polling itself is [`crate::library`]'s (`auto_import_ui.rs`).

use crate::theme;
use eframe::egui::{self, RichText};
use std::path::{Path, PathBuf};

pub const TITLE: &str = "Auto import";
pub const TEXT: &str = "Photos dropped in the watched folder (by tethering software or a card reader) are moved into the destination and imported, a few seconds after each one is written.";
pub const WATCHED: &str = "Watched folder";
pub const DESTINATION: &str = "Destination";
pub const CHOOSE: &str = "Choose…";
pub const NONE: &str = "None chosen";
pub const START: &str = "Start";
pub const STOP: &str = "Stop";
pub const CLOSE: &str = "Close";
pub const IN_CATALOGUE: &str =
    "The watched folder must not be a catalogued folder (photos are moved out of it).";

#[derive(Debug, Clone, PartialEq)]
pub enum AutoImportAction {
    None,
    Choose,
    Start { watch: PathBuf, folder: PathBuf },
    Stop,
    Close,
}

pub struct AutoImportDialog {
    pub watch: Option<PathBuf>,
    pub folder: Option<PathBuf>,
    /// The picker's ticket (its answer is taken only for the latest).
    pub ticket: u64,
}

/// Why `watch` cannot be watched (it is, or is inside, one of `folders`), if it cannot.
pub fn refused(watch: &Path, folders: &[PathBuf]) -> Option<&'static str> {
    folders
        .iter()
        .any(|f| watch.starts_with(f))
        .then_some(IN_CATALOGUE)
}

impl AutoImportDialog {
    pub fn new(watch: Option<PathBuf>, folder: Option<PathBuf>) -> AutoImportDialog {
        AutoImportDialog {
            watch,
            folder,
            ticket: 0,
        }
    }

    /// `folders`: the catalogued folders (the destinations); `running`: auto import is on.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        folders: &[PathBuf],
        running: bool,
    ) -> AutoImportAction {
        let mut action = AutoImportAction::None;
        let _ = egui::Modal::new(egui::Id::unique("auto-import")).show(ctx, |ui| {
            ui.set_max_width(460.0);
            ui.heading(TITLE);
            ui.label(RichText::new(TEXT).color(theme::MUTED));
            ui.add_space(8.0);
            egui::Grid::new("auto-import-grid")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label(WATCHED);
                    ui.horizontal(|ui| {
                        let shown = self
                            .watch
                            .as_ref()
                            .map_or(NONE.to_owned(), |p| p.display().to_string());
                        ui.label(shown);
                        if ui
                            .add_enabled(!running, egui::Button::new(CHOOSE))
                            .clicked()
                        {
                            action = AutoImportAction::Choose;
                        }
                    });
                    ui.end_row();
                    ui.label(DESTINATION);
                    let chosen = self
                        .folder
                        .as_ref()
                        .map_or(NONE.to_owned(), |p| p.display().to_string());
                    ui.add_enabled_ui(!running, |ui| {
                        let combo = egui::ComboBox::from_id_salt("auto-import-folder")
                            .selected_text(chosen)
                            .show_ui(ui, |ui| {
                                for f in folders {
                                    let on = self.folder.as_ref() == Some(f);
                                    if ui.selectable_label(on, f.display().to_string()).clicked() {
                                        self.folder = Some(f.clone());
                                    }
                                }
                            });
                        combo.response.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::ComboBox, true, DESTINATION)
                        });
                    });
                    ui.end_row();
                });
            let problem = self.watch.as_deref().and_then(|w| refused(w, folders));
            if let Some(p) = problem {
                ui.label(RichText::new(p).color(theme::DANGER));
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if running {
                    if ui.button(STOP).clicked() {
                        action = AutoImportAction::Stop;
                    }
                } else {
                    let ready = self.watch.is_some() && self.folder.is_some() && problem.is_none();
                    if ui.add_enabled(ready, egui::Button::new(START)).clicked()
                        && let (Some(watch), Some(folder)) = (&self.watch, &self.folder)
                    {
                        action = AutoImportAction::Start {
                            watch: watch.clone(),
                            folder: folder.clone(),
                        };
                    }
                }
                if ui.button(CLOSE).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    action = AutoImportAction::Close;
                }
            });
        });
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_catalogued_folder_or_one_inside_it_cannot_be_watched() {
        let folders = vec![PathBuf::from("/photos/2026")];
        assert!(refused(Path::new("/photos/2026"), &folders).is_some());
        assert!(refused(Path::new("/photos/2026/in"), &folders).is_some());
        assert!(refused(Path::new("/tether"), &folders).is_none());
    }
}
