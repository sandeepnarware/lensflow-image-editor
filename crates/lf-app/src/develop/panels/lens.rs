//! Lens corrections (desktop only, `desktop.lens`): distortion, the lens's own vignetting,
//! red/cyan and blue/yellow fringes, and vertical and horizontal perspective. Shown for every
//! photo, after the web's sections; its "Reset" puts every correction back to none.

use super::slider::{self, Range, SliderResponse};
use eframe::egui;
use lf_settings::EditSettings;
use lf_settings::lens::Lens;

/// The section's label.
pub const TITLE: &str = "Lens corrections";

/// The sliders' labels, in order.
pub const LABELS: [&str; 6] = [
    "Distortion",
    "Lens vignetting",
    "Red/cyan fringe",
    "Blue/yellow fringe",
    "Vertical perspective",
    "Horizontal perspective",
];

pub fn show(ui: &mut egui::Ui, s: &mut EditSettings) -> SliderResponse {
    let mut lens = Lens::from_settings(s);
    let mut out = SliderResponse::default();
    let values = [
        &mut lens.distortion,
        &mut lens.vignetting,
        &mut lens.ca_red,
        &mut lens.ca_blue,
        &mut lens.vertical,
        &mut lens.horizontal,
    ];
    for (label, value) in LABELS.iter().zip(values) {
        out = out.or(slider::edit(ui, label, value, Range::BIPOLAR, None));
    }
    if out.changed {
        lens.write_into(s);
    }
    out
}

/// The section's "Reset": no correction.
pub fn reset(s: &mut EditSettings) {
    Lens::default().write_into(s);
}
