//! Light: exposure and the five tone sliders.

use super::slider::{self, Range, SliderResponse};
use eframe::egui;
use lf_settings::EditSettings;

pub const EXPOSURE: Range = Range::new(-5.0, 5.0).step(0.05, 2);

/// The six sliders' labels, in the web's order.
pub const LABELS: [&str; 6] = [
    "Exposure",
    "Contrast",
    "Highlights",
    "Shadows",
    "Whites",
    "Blacks",
];

pub fn show(ui: &mut egui::Ui, s: &mut EditSettings) -> SliderResponse {
    let mut out = slider::edit(ui, LABELS[0], &mut s.exposure, EXPOSURE, None);
    let rest = [
        &mut s.contrast,
        &mut s.highlights,
        &mut s.shadows,
        &mut s.whites,
        &mut s.blacks,
    ];
    for (label, value) in LABELS[1..].iter().zip(rest) {
        out = out.or(slider::edit(ui, label, value, Range::BIPOLAR, None));
    }
    out
}
