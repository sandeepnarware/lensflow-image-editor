//! Film negative (desktop only, `desktop.negative`): "Convert negative" measures the film
//! base and the photo's range from the photo itself ([`lf_settings::negative`]) and turns the
//! negative into a positive before every other adjustment; then white balance, exposure and
//! the rest work on the positive. Shown for every photo, after the web's sections; its "Reset"
//! (or "Off") takes the conversion away.

use super::slider::SliderResponse;
use crate::develop::auto::Sample;
use crate::theme;
use eframe::egui::{self, RichText};
use lf_settings::EditSettings;
use lf_settings::negative::Negative;

pub const TITLE: &str = "Film negative";
pub const CONVERT: &str = "Convert negative";
pub const MEASURE_AGAIN: &str = "Measure again";
pub const OFF: &str = "Off";
pub const ON: &str =
    "Converted from a negative. White balance and the tone sliders now work on the positive.";
pub const ABOUT: &str = "For a scanned or photographed colour or black-and-white negative: the film's base and range are measured from the photo.";
pub const CANNOT: &str = "This photo does not look like a negative to measure (too flat).";

pub fn show(ui: &mut egui::Ui, s: &mut EditSettings, sample: Option<&Sample>) -> SliderResponse {
    let mut out = SliderResponse::default();
    let on = Negative::from_settings(s).is_some();
    let note = egui::Id::unique("negative-note");
    ui.label(
        RichText::new(if on { ON } else { ABOUT })
            .color(theme::MUTED)
            .small(),
    );
    ui.horizontal(|ui| {
        let label = if on { MEASURE_AGAIN } else { CONVERT };
        if ui
            .add_enabled(sample.is_some(), egui::Button::new(label))
            .clicked()
            && let Some(sample) = sample
        {
            let measured = Negative::measure(&sample.rgb);
            ui.ctx()
                .data_mut(|d| d.insert_temp(note, measured.is_none()));
            if let Some(n) = measured {
                Negative::write_into(Some(&n), s);
                out = SliderResponse {
                    changed: true,
                    released: true,
                };
            }
        }
        if on && ui.button(OFF).clicked() {
            Negative::write_into(None, s);
            out = SliderResponse {
                changed: true,
                released: true,
            };
        }
    });
    if ui.ctx().data(|d| d.get_temp::<bool>(note)).unwrap_or(false) {
        ui.label(RichText::new(CANNOT).color(theme::WARNING).small());
    }
    out
}

/// The section's "Reset": no conversion.
pub fn reset(s: &mut EditSettings) {
    Negative::write_into(None, s);
}
