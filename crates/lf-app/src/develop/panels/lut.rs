//! The LUT look (desktop only, `desktop.lut`): a 3D LUT from the catalogue's LUT folder and
//! how much of it, and "Import .cube…" to add one. A photo whose LUT is not loaded here
//! (another machine, a deleted file) says so and renders without it.

use super::slider::{self, Range, SliderResponse};
use crate::theme;
use eframe::egui::{self, RichText};
use lf_settings::EditSettings;
use lf_settings::lut::LutChoice;

pub const TITLE: &str = "LUT look";
pub const LIST: &str = "LUT";
pub const NONE: &str = "None";
pub const AMOUNT: &str = "Amount";
pub const IMPORT: &str = "Import .cube…";
pub const NOT_LOADED: &str = "This LUT is not in this catalogue; the photo shows without it.";
pub const NO_LUTS: &str = "No LUTs yet. Import a .cube file to use it as a look.";

/// The section; also says whether "Import .cube…" was clicked.
pub fn show(ui: &mut egui::Ui, s: &mut EditSettings, names: &[String]) -> (SliderResponse, bool) {
    let mut out = SliderResponse::default();
    let mut choice = LutChoice::from_settings(s);
    let shown = choice.as_ref().map_or(NONE, |c| c.name.as_str()).to_owned();
    let mut picked: Option<Option<String>> = None;
    let combo = egui::ComboBox::from_id_salt("lut-look")
        .selected_text(&shown)
        .width(170.0)
        .show_ui(ui, |ui| {
            if ui.selectable_label(choice.is_none(), NONE).clicked() {
                picked = Some(None);
            }
            for name in names {
                if ui.selectable_label(shown == *name, name).clicked() {
                    picked = Some(Some(name.clone()));
                }
            }
        });
    combo
        .response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, LIST));
    if let Some(p) = picked {
        let amount = choice.as_ref().map_or(100.0, |c| c.amount);
        choice = p.map(|name| LutChoice { name, amount });
        out = SliderResponse {
            changed: true,
            released: true,
        };
    }
    if let Some(c) = &mut choice {
        if !names.contains(&c.name) {
            ui.label(RichText::new(NOT_LOADED).color(theme::WARNING));
        }
        out = out.or(slider::edit(ui, AMOUNT, &mut c.amount, Range::AMOUNT, None));
    } else if names.is_empty() {
        ui.label(RichText::new(NO_LUTS).color(theme::MUTED));
    }
    if out.changed {
        LutChoice::write_into(choice.as_ref(), s);
    }
    let import = ui.button(IMPORT).clicked();
    (out, import)
}

/// The section's "Reset": no look.
pub fn reset(s: &mut EditSettings) {
    LutChoice::write_into(None, s);
}
