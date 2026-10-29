//! RAW (shown for RAW photos only): the camera profile (built-in colour or a DNG `.dcp`
//! imported into the catalogue, [`crate::profiles`]), the base curve and what happens to
//! clipped highlights. They change how the sensor data is developed, so a change commits and
//! the photo is opened again through the frame cache (whose key includes them).

use super::slider::SliderResponse;
use crate::theme;
use eframe::egui::{self, RichText};
use lf_settings::{DevelopSettings, EditSettings, Highlights, Profile};

pub const PROFILE: &str = "Profile";
pub const HIGHLIGHTS: &str = "Highlights";
pub const CAMERA_PROFILE: &str = "Camera profile";
pub const BUILT_IN: &str = "Built-in";
pub const IMPORT: &str = "Import .dcp…";
pub const NOT_LOADED: &str =
    "This camera profile is not in this catalogue; the photo shows with the built-in colour.";
pub const OWN_CURVE: &str =
    "This profile has its own tone curve, used in place of the profile curve.";

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

/// The section; also says whether "Import .dcp…" was clicked. `profiles` are the loaded
/// camera profiles' names and cameras.
pub fn show(
    ui: &mut egui::Ui,
    s: &mut EditSettings,
    profiles: &[(String, String)],
) -> (SliderResponse, bool) {
    let chosen = DevelopSettings::dcp_name(s);
    let shown = chosen.clone().unwrap_or_else(|| BUILT_IN.to_owned());
    let mut picked: Option<Option<String>> = None;
    ui.horizontal(|ui| {
        ui.label(CAMERA_PROFILE);
        let combo = egui::ComboBox::from_id_salt("camera-profile")
            .selected_text(&shown)
            .width(160.0)
            .show_ui(ui, |ui| {
                if ui.selectable_label(chosen.is_none(), BUILT_IN).clicked() {
                    picked = Some(None);
                }
                for (name, camera) in profiles {
                    let label = if camera.is_empty() {
                        name.clone()
                    } else {
                        format!("{name} ({camera})")
                    };
                    if ui.selectable_label(shown == *name, label).clicked() {
                        picked = Some(Some(name.clone()));
                    }
                }
            });
        combo
            .response
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, CAMERA_PROFILE));
    });
    let mut changed = false;
    if let Some(p) = picked
        && p != chosen
    {
        DevelopSettings::set_dcp(s, p.as_deref());
        changed = true;
    }
    if let Some(name) = DevelopSettings::dcp_name(s) {
        match profiles.iter().find(|(n, _)| n.eq_ignore_ascii_case(&name)) {
            None => {
                ui.label(RichText::new(NOT_LOADED).color(theme::WARNING).small());
            }
            Some((n, _)) => {
                if lf_raw::dcp::get(lf_settings::dcp_id(n)).is_some_and(|d| d.has_tone_curve()) {
                    ui.label(RichText::new(OWN_CURVE).color(theme::MUTED).small());
                }
            }
        }
    }
    let import = ui.button(IMPORT).clicked();
    let mut develop = DevelopSettings::from_settings(s);
    changed |= segmented(
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
    (
        SliderResponse {
            changed,
            released: changed,
        },
        import,
    )
}

/// The section's "Reset": the default develop settings.
pub fn reset(s: &mut EditSettings) {
    DevelopSettings::default().write_into(s);
}
