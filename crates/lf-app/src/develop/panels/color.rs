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

/// The light a photo without a known white balance (a JPEG, say) is taken to be balanced for.
pub const ASSUMED_KELVIN: f64 = 5500.0;

/// The sliders for preset `kelvin` on a photo shot for `as_shot` kelvin, rounded as the
/// sliders keep them.
pub fn preset_wb(as_shot: f64, kelvin: f64) -> Option<(f64, f64)> {
    lf_settings::shader_values::kelvin_wb(as_shot, kelvin).map(|(t, i)| (t.round(), i.round()))
}

/// What the white-balance list shows for the current sliders: As shot, a preset whose values
/// they hold, or Custom.
pub fn wb_name(s: &EditSettings, as_shot: f64) -> &'static str {
    if s.temperature == 0.0 && s.tint == 0.0 {
        return AS_SHOT;
    }
    lf_settings::shader_values::WB_PRESETS
        .iter()
        .find(|(_, k)| preset_wb(as_shot, *k) == Some((s.temperature, s.tint)))
        .map_or(CUSTOM_WB, |(name, _)| name)
}

/// Draws the section. Returns what the sliders did, and whether the eyedropper was clicked
/// (the Develop view opens its tool).
pub fn show(
    ui: &mut egui::Ui,
    s: &mut EditSettings,
    sample: Option<&Sample>,
    picking: bool,
    as_shot: Option<f64>,
) -> (SliderResponse, bool) {
    let shot = as_shot.unwrap_or(ASSUMED_KELVIN);
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
            .width(96.0)
            .selected_text(wb_name(s, shot))
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(wb_name(s, shot) == AS_SHOT, AS_SHOT)
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
                ui.separator();
                for (name, kelvin) in lf_settings::shader_values::WB_PRESETS {
                    let values = preset_wb(shot, kelvin);
                    if ui
                        .selectable_label(wb_name(s, shot) == name, name)
                        .on_hover_text(format!("{kelvin:.0} K"))
                        .clicked()
                    {
                        chosen = Some(values);
                    }
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
    if let Some(k) = as_shot {
        ui.label(
            egui::RichText::new(format!("As shot: about {:.0} K", (k / 50.0).round() * 50.0))
                .small()
                .color(crate::theme::MUTED),
        );
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
