//! Detail: clarity, texture, dehaze, sharpening (its radius once it is on) and noise
//! reduction.

use super::slider::{self, Range, SliderResponse};
use crate::theme;
use eframe::egui::{self, RichText};
use lf_settings::EditSettings;

pub const SHARPEN: Range = Range::new(0.0, 150.0);
pub const RADIUS: Range = Range::new(0.5, 3.0).step(0.1, 1).neutral(1.0);
pub const NOTE: &str =
    "Zoom in to judge sharpening and noise: the preview is smaller than the photo.";

pub fn show(ui: &mut egui::Ui, s: &mut EditSettings) -> SliderResponse {
    let mut out = SliderResponse::default();
    for (label, value) in [
        ("Clarity", &mut s.clarity),
        ("Texture", &mut s.texture),
        ("Dehaze", &mut s.dehaze),
    ] {
        out = out.or(slider::edit(ui, label, value, Range::BIPOLAR, None));
    }
    out = out.or(slider::edit(
        ui,
        "Sharpening",
        &mut s.sharpen,
        SHARPEN,
        None,
    ));
    if s.sharpen > 0.0 {
        out = out.or(slider::edit(
            ui,
            "Sharpening radius",
            &mut s.sharpen_radius,
            RADIUS,
            None,
        ));
    }
    out = out.or(slider::edit(
        ui,
        "Noise reduction",
        &mut s.noise,
        Range::AMOUNT,
        None,
    ));
    out = out.or(slider::edit(
        ui,
        "Colour noise reduction",
        &mut s.color_noise,
        Range::AMOUNT,
        None,
    ));
    ui.label(RichText::new(NOTE).small().color(theme::MUTED));
    out
}
