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

/// Upright's buttons and its note when it found too few lines.
pub const UPRIGHT: &str = "Upright";
pub const PROFILE: &str = "Apply lens profile";
pub const UPRIGHT_VERTICAL: &str = "Vertical";
pub const UPRIGHT_FULL: &str = "Full";
pub const UPRIGHT_NONE: &str =
    "Not enough straight lines were found to straighten (or the photo is turned or flipped).";

pub fn show(
    ui: &mut egui::Ui,
    s: &mut EditSettings,
    sample: Option<&crate::develop::auto::Sample>,
    profile_asked: &mut bool,
) -> SliderResponse {
    let mut out = SliderResponse::default();
    if ui
        .button(PROFILE)
        .on_hover_text(
            "Distortion, lens vignetting and fringes for this lens, from the lensfun database",
        )
        .clicked()
    {
        *profile_asked = true;
    }
    // Upright: perspective and straightening fitted to the photo's lines.
    let note_id = egui::Id::unique("upright-note");
    ui.horizontal(|ui| {
        ui.label(UPRIGHT)
            .on_hover_text("Make vertical lines upright and the horizon level");
        for (label, mode) in [
            (UPRIGHT_VERTICAL, lf_settings::upright::Mode::Vertical),
            (UPRIGHT_FULL, lf_settings::upright::Mode::Full),
        ] {
            if ui
                .add_enabled(sample.is_some(), egui::Button::new(label))
                .clicked()
                && let Some(sample) = sample
            {
                let applied = upright(s, sample, mode);
                ui.ctx().data_mut(|d| d.insert_temp(note_id, !applied));
                if applied {
                    out.changed = true;
                    out.released = true;
                }
            }
        }
    });
    if ui
        .ctx()
        .data(|d| d.get_temp::<bool>(note_id))
        .unwrap_or(false)
    {
        ui.label(
            egui::RichText::new(UPRIGHT_NONE)
                .small()
                .color(crate::theme::WARNING),
        );
    }
    let mut lens = Lens::from_settings(s);
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

/// Fits Upright to the lines in `sample` and applies it (perspective and straightening, the
/// crop kept inside); false when there were too few lines.
pub fn upright(
    s: &mut EditSettings,
    sample: &crate::develop::auto::Sample,
    mode: lf_settings::upright::Mode,
) -> bool {
    // The fit works in the photo as shot; a quarter turn or a flip would mix up its angle.
    if s.rotate != lf_settings::Rotate::R0 || s.flip_h || s.flip_v {
        return false;
    }
    let segments = crate::develop::upright::segments(sample);
    let aspect = f64::from(sample.width) / f64::from(sample.height.max(1));
    let mut lens = Lens::from_settings(s);
    let Some(fit) = lf_settings::upright::fit(&segments, aspect, &lens, mode) else {
        return false;
    };
    lens.vertical = fit.vertical;
    lens.horizontal = fit.horizontal;
    lens.write_into(s);
    let (tw, th) = crate::develop::tools::crop::turned(s, sample.width, sample.height);
    crate::develop::tools::crop::straighten(s, fit.angle, tw, th);
    true
}
