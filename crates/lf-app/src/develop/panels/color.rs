//! Colour: black & white, white balance, vibrance and saturation (hidden in black & white).

use super::slider::{self, Range, SliderResponse, Track};
use eframe::egui;
use lf_settings::EditSettings;

pub const BW: &str = "Black & white";

pub fn show(ui: &mut egui::Ui, s: &mut EditSettings) -> SliderResponse {
    let mut out = SliderResponse::default();
    if ui.checkbox(&mut s.bw, BW).clicked() {
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
    out
}
