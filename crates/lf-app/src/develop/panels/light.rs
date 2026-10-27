//! Light: Auto tone, exposure and the five tone sliders.

use super::slider::{self, Range, SliderResponse};
use crate::develop::auto::{self, Sample};
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

/// The button that sets the six sliders from the photo.
pub const AUTO_TONE: &str = "Auto tone";

pub fn show(ui: &mut egui::Ui, s: &mut EditSettings, sample: Option<&Sample>) -> SliderResponse {
    let mut out = SliderResponse::default();
    let button = ui
        .add_enabled(sample.is_some(), egui::Button::new(AUTO_TONE))
        .on_hover_text("Set exposure and tone from the photo (Ctrl+U)");
    if button.clicked()
        && let Some(tone) = sample.and_then(|sample| auto::auto_tone(sample, s))
    {
        tone.apply(s);
        out = SliderResponse {
            changed: true,
            released: true,
        };
    }
    out = out.or(slider::edit(ui, LABELS[0], &mut s.exposure, EXPOSURE, None));
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
