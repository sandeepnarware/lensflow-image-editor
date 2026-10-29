//! Lens blur (desktop only, `desktop.lensBlur`): "Find depth (AI)" finds the photo's depth once
//! (Depth Anything, on this computer); then Amount, Focus (the depth kept sharp: 0 far, 100
//! near) and Range (how deep the sharp band is) blur the rest live, as any other slider.
//! "Reset" takes the blur (and its depth) away.

use super::slider::{self, Range, SliderResponse};
use crate::theme;
use eframe::egui::{self, RichText};
use lf_settings::EditSettings;
use lf_settings::lens_blur::LensBlur;

pub const TITLE: &str = "Lens blur";
pub const FIND: &str = "Find depth (AI)";
pub const FIND_AGAIN: &str = "Find depth again";
pub const FIRST: &str = "Find the photo's depth first; then the blur follows the sliders.";
pub const AI_OFF: &str = "The AI tools need the studio in good standing.";
pub const LABELS: [&str; 3] = ["Amount", "Focus", "Range"];

/// The section; `.1` is true when a depth was asked for.
pub fn show(ui: &mut egui::Ui, s: &mut EditSettings, ai_allowed: bool) -> (SliderResponse, bool) {
    let mut out = SliderResponse::default();
    let Some(mut b) = LensBlur::from_settings(s) else {
        ui.label(RichText::new(FIRST).color(theme::MUTED).small());
        let find = ui
            .add_enabled(ai_allowed, egui::Button::new(FIND))
            .on_disabled_hover_text(AI_OFF)
            .clicked();
        return (out, find);
    };
    out = out.or(slider::edit(
        ui,
        LABELS[0],
        &mut b.amount,
        Range::AMOUNT,
        None,
    ));
    out = out.or(slider::edit(
        ui,
        LABELS[1],
        &mut b.focus,
        Range::AMOUNT,
        None,
    ));
    out = out.or(slider::edit(
        ui,
        LABELS[2],
        &mut b.range,
        Range::AMOUNT,
        None,
    ));
    if out.changed {
        LensBlur::write_into(Some(&b), s);
    }
    let find = ui
        .add_enabled(ai_allowed, egui::Button::new(FIND_AGAIN))
        .on_disabled_hover_text(AI_OFF)
        .on_hover_text("For a photo whose crop or content changed")
        .clicked();
    (out, find)
}

/// The section's "Reset": no blur.
pub fn reset(s: &mut EditSettings) {
    LensBlur::write_into(None, s);
}
