//! Effects: vignette (its midpoint once it is on) and grain (its size once it is on).

use super::slider::{self, Range, SliderResponse};
use eframe::egui;
use lf_settings::EditSettings;

pub const MIDPOINT: Range = Range::AMOUNT.neutral(50.0);
pub const GRAIN_SIZE: Range = Range::AMOUNT.neutral(25.0);

pub fn show(ui: &mut egui::Ui, s: &mut EditSettings) -> SliderResponse {
    let mut out = slider::edit(ui, "Vignette", &mut s.vignette, Range::BIPOLAR, None);
    if s.vignette != 0.0 {
        out = out.or(slider::edit(
            ui,
            "Vignette midpoint",
            &mut s.vignette_midpoint,
            MIDPOINT,
            None,
        ));
    }
    out = out.or(slider::edit(ui, "Grain", &mut s.grain, Range::AMOUNT, None));
    if s.grain > 0.0 {
        out = out.or(slider::edit(
            ui,
            "Grain size",
            &mut s.grain_size,
            GRAIN_SIZE,
            None,
        ));
    }
    out
}
