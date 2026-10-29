//! Point colour (desktop only, `desktop.pointColour`, as Lightroom's Point Color): "Pick a
//! colour" opens a picker (a click on the photo adds the colour there, [`crate::develop::tools`]);
//! each picked colour shows as a swatch, and the chosen one has its hue, saturation and
//! luminance shifts, its range and Remove. Up to four colours.

use super::slider::{self, Range, SliderResponse};
use crate::theme;
use eframe::egui::{self, Color32, RichText};
use lf_settings::EditSettings;
use lf_settings::point_colour::{MAX_POINTS, Points};

pub const TITLE: &str = "Point colour";
pub const PICK: &str = "🎯 Pick a colour";
pub const PICK_HINT: &str = "Click a colour in the photo to add it.";
pub const FULL: &str = "Up to four colours; remove one to pick another.";
pub const REMOVE: &str = "Remove colour";
pub const LABELS: [&str; 4] = ["Hue shift", "Saturation shift", "Luminance shift", "Range"];

/// A picked colour's swatch.
pub fn swatch(p: &lf_settings::point_colour::Point) -> Color32 {
    let hsva = egui::ecolor::Hsva::new(
        (p.hue / 360.0) as f32,
        p.saturation as f32,
        (p.brightness.max(0.2) * 1.2).min(1.0) as f32,
        1.0,
    );
    Color32::from(hsva)
}

/// The section; `.1` is true when "Pick a colour" was clicked (`picking`: the picker is on).
pub fn show(
    ui: &mut egui::Ui,
    s: &mut EditSettings,
    selected: &mut Option<usize>,
    picking: bool,
) -> (SliderResponse, bool) {
    let mut points = Points::from_settings(s);
    let mut out = SliderResponse::default();
    let full = points.0.len() >= MAX_POINTS;
    let pick = ui
        .add_enabled(!full || picking, egui::Button::selectable(picking, PICK))
        .on_disabled_hover_text(FULL)
        .clicked();
    if picking {
        ui.label(RichText::new(PICK_HINT).color(theme::MUTED));
    }
    if selected.is_some_and(|i| i >= points.0.len()) {
        *selected = points.0.len().checked_sub(1);
    }
    ui.horizontal_wrapped(|ui| {
        for (i, p) in points.0.iter().enumerate() {
            let text = RichText::new(format!("■ {}", i + 1)).color(swatch(p));
            if ui.selectable_label(*selected == Some(i), text).clicked() {
                *selected = Some(i);
            }
        }
    });
    let Some(i) = *selected else {
        return (out, pick);
    };
    let p = &mut points.0[i];
    out = out.or(slider::edit(
        ui,
        LABELS[0],
        &mut p.hue_shift,
        Range::BIPOLAR,
        None,
    ));
    out = out.or(slider::edit(
        ui,
        LABELS[1],
        &mut p.saturation_shift,
        Range::BIPOLAR,
        None,
    ));
    out = out.or(slider::edit(
        ui,
        LABELS[2],
        &mut p.luminance_shift,
        Range::BIPOLAR,
        None,
    ));
    out = out.or(slider::edit(
        ui,
        LABELS[3],
        &mut p.range,
        Range::AMOUNT,
        None,
    ));
    if ui.button(REMOVE).clicked() {
        points.0.remove(i);
        *selected = points.0.len().checked_sub(1).map(|last| i.min(last));
        out = SliderResponse {
            changed: true,
            released: true,
        };
    }
    if out.changed {
        points.write_into(s);
    }
    (out, pick)
}

/// The section's "Reset": no colours.
pub fn reset(s: &mut EditSettings) {
    Points::default().write_into(s);
}
