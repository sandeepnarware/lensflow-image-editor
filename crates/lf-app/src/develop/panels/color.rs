//! Colour: black & white, white balance (as shot, auto, the eyedropper and the two
//! sliders), vibrance and saturation (hidden in black & white).

use super::slider::{self, Range, SliderResponse, Track};
use crate::develop::auto::{self, Sample};
use eframe::egui;
use lf_settings::EditSettings;

pub const BW: &str = "Black & white";
/// The white-balance list's name.
pub const WHITE_BALANCE: &str = "White balance";
/// The white-balance choices.
pub const AS_SHOT: &str = "As shot";
pub const AUTO_WB: &str = "Auto";
pub const CUSTOM_WB: &str = "Custom";
/// The eyedropper's button.
pub const PICK_WB: &str = "Eyedropper";

/// What the white-balance list shows for the current sliders.
pub fn wb_name(s: &EditSettings) -> &'static str {
    if s.temperature == 0.0 && s.tint == 0.0 {
        AS_SHOT
    } else {
        CUSTOM_WB
    }
}

/// Draws the section. Returns what the sliders did, and whether the eyedropper was clicked
/// (the Develop view opens its tool).
pub fn show(
    ui: &mut egui::Ui,
    s: &mut EditSettings,
    sample: Option<&Sample>,
    picking: bool,
) -> (SliderResponse, bool) {
    let mut out = SliderResponse::default();
    let mut pick = false;
    if ui.checkbox(&mut s.bw, BW).clicked() {
        out.changed = true;
        out.released = true;
    }
    let mut chosen = None;
    ui.horizontal(|ui| {
        // Short, so the row fits the side panel (the list itself is named in full).
        ui.label("WB").on_hover_text(WHITE_BALANCE);
        let list = egui::ComboBox::from_id_salt("white-balance")
            .width(80.0)
            .selected_text(wb_name(s))
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(wb_name(s) == AS_SHOT, AS_SHOT)
                    .clicked()
                {
                    chosen = Some(Some((0.0, 0.0)));
                }
                let auto_button = ui
                    .add_enabled(sample.is_some(), egui::Button::selectable(false, AUTO_WB))
                    .on_hover_text("Ctrl+Shift+U");
                if auto_button.clicked() {
                    chosen = Some(sample.and_then(auto::auto_wb));
                }
            });
        list.response
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, WHITE_BALANCE));
        let dropper = ui
            .add_enabled(sample.is_some(), egui::Button::selectable(picking, PICK_WB))
            .on_hover_text("Click something in the photo that should be grey or white (W)");
        if dropper.clicked() {
            pick = true;
        }
    });
    if let Some(Some((temperature, tint))) = chosen
        && (s.temperature, s.tint) != (temperature, tint)
    {
        s.temperature = temperature;
        s.tint = tint;
        out.changed = true;
        out.released = true;
    }
    out = out.or(slider::edit(
        ui,
        "Temperature",
        &mut s.temperature,
        Range::BIPOLAR,
        Some(Track::Temperature),
    ));
    out = out.or(slider::edit(
        ui,
        "Tint",
        &mut s.tint,
        Range::BIPOLAR,
        Some(Track::Tint),
    ));
    if !s.bw {
        out = out.or(slider::edit(
            ui,
            "Vibrance",
            &mut s.vibrance,
            Range::BIPOLAR,
            None,
        ));
        out = out.or(slider::edit(
            ui,
            "Saturation",
            &mut s.saturation,
            Range::BIPOLAR,
            None,
        ));
    }
    (out, pick)
}
