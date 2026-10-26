//! RAW (shown for RAW photos only): the profile and what happens to clipped highlights. They
//! change how the sensor data is developed, so a change commits and the photo is opened
//! again through the frame cache (whose key includes them).

use super::slider::SliderResponse;
use eframe::egui;
use lf_settings::{DevelopSettings, EditSettings, Highlights, Profile};

pub const PROFILE: &str = "Profile";
pub const HIGHLIGHTS: &str = "Highlights";

fn segmented<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut T,
    choices: &[(T, &str)],
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for (choice, text) in choices.iter().rev() {
                if ui.selectable_label(*value == *choice, *text).clicked() && *value != *choice {
                    *value = *choice;
                    changed = true;
                }
            }
        });
    });
    changed
}

pub fn show(ui: &mut egui::Ui, s: &mut EditSettings) -> SliderResponse {
    let mut develop = DevelopSettings::from_settings(s);
    let mut changed = segmented(
        ui,
        PROFILE,
        &mut develop.profile,
        &[(Profile::Camera, "Camera"), (Profile::Linear, "Linear")],
    );
    changed |= segmented(
        ui,
        HIGHLIGHTS,
        &mut develop.highlights,
        &[
            (Highlights::Clip, "Clip"),
            (Highlights::Reconstruct, "Reconstruct"),
        ],
    );
    if changed {
        develop.write_into(s);
    }
    SliderResponse {
        changed,
        released: changed,
    }
}

/// The section's "Reset": the default develop settings.
pub fn reset(s: &mut EditSettings) {
    DevelopSettings::default().write_into(s);
}
