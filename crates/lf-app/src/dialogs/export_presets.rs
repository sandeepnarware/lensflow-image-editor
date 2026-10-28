//! Export presets: the export form saved under a name ("Client web 2048 px + watermark"), kept
//! in `prefs.export_presets`, plus a few built in. Choosing one fills the form; a preset saved
//! without a folder keeps the folder already chosen. The bar sits at the top of the export
//! form ([`super::export`]).

use super::export::ExportForm;
use eframe::egui::{self, Key};
use lf_catalog::export::{Destination, ExportSpec, Format, Size};
use lf_catalog::finish::Sharpen;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const PRESET: &str = "Preset";
pub const SAVE_AS: &str = "Save as preset…";
pub const DELETE: &str = "Delete preset";
pub const PRESET_NAME: &str = "Preset name";
/// The longest preset name kept, in characters.
pub const MAX_NAME: usize = 60;

/// A saved export form: the `ExportSpec` JSON (its folder may be empty: keep the form's).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportPreset {
    pub name: String,
    pub spec: serde_json::Value,
}

/// The presets every studio has, before its own.
pub fn built_in() -> Vec<(&'static str, ExportSpec)> {
    let spec = |format, size, sharpen| ExportSpec {
        format,
        size,
        destination: Destination::Folder(PathBuf::new()),
        rename: lf_catalog::export::Pattern(super::export::DEFAULT_PATTERN.to_owned()),
        metadata: lf_catalog::export::Metadata::All,
        overwrite: false,
        sharpen,
        watermark: None,
    };
    vec![
        (
            "Web (2048 px JPEG)",
            spec(
                Format::Jpeg { quality: 85 },
                Size::LongEdge(2048),
                Sharpen::Standard,
            ),
        ),
        (
            "Full size JPEG",
            spec(Format::Jpeg { quality: 93 }, Size::Original, Sharpen::Off),
        ),
        (
            "Print (16-bit TIFF)",
            spec(Format::Tiff16, Size::Original, Sharpen::Off),
        ),
    ]
}

/// Fills `form` from `spec`, keeping the form's folder when the preset has none.
pub fn apply(form: &mut ExportForm, spec: &ExportSpec) {
    let folder = form.folder.clone();
    *form = ExportForm::from_spec(spec);
    let Destination::Folder(saved) = &spec.destination;
    if saved.as_os_str().is_empty() {
        form.folder = folder;
    }
}

/// `form` as a preset called `name`, or why not. A form without a folder makes a preset
/// that keeps whatever folder is chosen when it is used.
pub fn save(form: &ExportForm, name: &str) -> Result<ExportPreset, &'static str> {
    let name: String = name.trim().chars().take(MAX_NAME).collect();
    if name.is_empty() {
        return Err("Type a name for the preset.");
    }
    let mut form = form.clone();
    form.folder.get_or_insert_with(PathBuf::new);
    let spec = form.spec()?;
    let spec = serde_json::to_value(&spec).map_err(|_| "The preset could not be saved.")?;
    Ok(ExportPreset { name, spec })
}

/// The bar's own state: the preset last chosen and the name being typed for a new one.
#[derive(Debug, Clone, Default)]
pub struct PresetBar {
    pub chosen: Option<String>,
    pub naming: Option<String>,
    pub error: Option<&'static str>,
}

impl PresetBar {
    /// The bar: choose, save as, delete. Returns the new list of the studio's presets when it
    /// changed (to be saved in prefs).
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        presets: &[ExportPreset],
        form: &mut ExportForm,
    ) -> Option<Vec<ExportPreset>> {
        let mut changed = None;
        ui.horizontal(|ui| {
            ui.label(PRESET);
            let shown = self.chosen.clone().unwrap_or_else(|| "Choose…".to_owned());
            let combo = egui::ComboBox::from_id_salt("export-preset")
                .selected_text(shown)
                .width(220.0)
                .show_ui(ui, |ui| {
                    for (name, spec) in built_in() {
                        if ui.selectable_label(false, name).clicked() {
                            apply(form, &spec);
                            self.chosen = Some(name.to_owned());
                        }
                    }
                    if !presets.is_empty() {
                        ui.separator();
                    }
                    for p in presets {
                        if ui.selectable_label(false, &p.name).clicked() {
                            match serde_json::from_value::<ExportSpec>(p.spec.clone()) {
                                Ok(spec) => {
                                    apply(form, &spec);
                                    self.chosen = Some(p.name.clone());
                                }
                                Err(e) => {
                                    log::warn!("export preset {} is unreadable: {e}", p.name);
                                    self.error = Some("That preset could not be read.");
                                }
                            }
                        }
                    }
                });
            combo
                .response
                .widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, PRESET));
            if ui.button(SAVE_AS).clicked() {
                self.naming = Some(self.chosen.clone().unwrap_or_default());
                self.error = None;
            }
            let own = self
                .chosen
                .as_ref()
                .is_some_and(|c| presets.iter().any(|p| &p.name == c));
            if own && ui.button(DELETE).clicked() {
                let gone = self.chosen.take();
                changed = Some(
                    presets
                        .iter()
                        .filter(|p| Some(&p.name) != gone.as_ref())
                        .cloned()
                        .collect(),
                );
            }
        });
        if let Some(name) = &mut self.naming {
            let mut done = false;
            let mut cancel = false;
            ui.horizontal(|ui| {
                let field = ui.add(
                    egui::TextEdit::singleline(name)
                        .hint_text("Client web 2048 px + watermark")
                        .desired_width(240.0),
                );
                field.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::TextInput, true, PRESET_NAME)
                });
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                if ui.button("Save").clicked() || enter {
                    done = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
            if done {
                match save(form, name) {
                    Ok(preset) => {
                        // A preset of the same name is replaced.
                        let mut list: Vec<ExportPreset> = presets
                            .iter()
                            .filter(|p| !p.name.eq_ignore_ascii_case(&preset.name))
                            .cloned()
                            .collect();
                        self.chosen = Some(preset.name.clone());
                        list.push(preset);
                        list.sort_by_key(|p| p.name.to_lowercase());
                        changed = Some(list);
                        self.naming = None;
                        self.error = None;
                    }
                    Err(e) => self.error = Some(e),
                }
            } else if cancel {
                self.naming = None;
                self.error = None;
            }
        }
        if let Some(e) = self.error {
            ui.label(egui::RichText::new(e).color(crate::theme::WARNING).small());
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialogs::export::SizeKind;

    #[test]
    fn a_preset_without_a_folder_keeps_the_chosen_one() {
        let mut form = ExportForm {
            size: SizeKind::LongEdge,
            pixels: 1600,
            watermark: true,
            ..ExportForm::default()
        };
        let preset = save(&form, "  Client web ").unwrap();
        assert_eq!(preset.name, "Client web");
        form = ExportForm {
            folder: Some("D:/out".into()),
            ..ExportForm::default()
        };
        let spec: ExportSpec = serde_json::from_value(preset.spec).unwrap();
        apply(&mut form, &spec);
        assert_eq!(form.folder, Some(PathBuf::from("D:/out")));
        assert_eq!(
            (form.size, form.pixels, form.watermark),
            (SizeKind::LongEdge, 1600, true)
        );
        assert!(save(&form, "  ").is_err());
    }

    #[test]
    fn a_preset_with_a_folder_brings_it() {
        let form = ExportForm {
            folder: Some("D:/web".into()),
            ..ExportForm::default()
        };
        let preset = save(&form, "Web").unwrap();
        let mut other = ExportForm {
            folder: Some("D:/else".into()),
            ..ExportForm::default()
        };
        apply(&mut other, &serde_json::from_value(preset.spec).unwrap());
        assert_eq!(other.folder, Some(PathBuf::from("D:/web")));
    }

    #[test]
    fn the_built_in_presets_make_valid_forms() {
        for (name, spec) in built_in() {
            let mut form = ExportForm {
                folder: Some("D:/out".into()),
                ..ExportForm::default()
            };
            apply(&mut form, &spec);
            assert!(form.spec().is_ok(), "{name}");
        }
    }
}
