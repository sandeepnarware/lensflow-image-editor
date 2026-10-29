//! Colour calibration (desktop only, `desktop.calibration`): the shadows' green–magenta tint
//! and the hue and saturation of the red, green and blue primaries, as Lightroom's
//! Calibration panel. Shown for every photo, after the web's sections; its "Reset" puts
//! everything back to neutral.

use super::slider::{self, Range, SliderResponse};
use eframe::egui;
use lf_settings::EditSettings;
use lf_settings::calibration::Calibration;

/// The section's label.
pub const TITLE: &str = "Calibration";

/// The sliders' labels, in order.
pub const LABELS: [&str; 7] = [
    "Shadows tint",
    "Red primary hue",
    "Red primary saturation",
    "Green primary hue",
    "Green primary saturation",
    "Blue primary hue",
    "Blue primary saturation",
];

pub fn show(ui: &mut egui::Ui, s: &mut EditSettings) -> SliderResponse {
    let mut out = SliderResponse::default();
    let mut c = Calibration::from_settings(s);
    let values = [
        &mut c.shadows_tint,
        &mut c.red_hue,
        &mut c.red_saturation,
        &mut c.green_hue,
        &mut c.green_saturation,
        &mut c.blue_hue,
        &mut c.blue_saturation,
    ];
    for (label, value) in LABELS.iter().zip(values) {
        out = out.or(slider::edit(ui, label, value, Range::BIPOLAR, None));
    }
    if out.changed {
        c.write_into(s);
    }
    out
}

/// The section's "Reset": neutral.
pub fn reset(s: &mut EditSettings) {
    Calibration::default().write_into(s);
}
