//! Colour mix: one slider per colour band for the chosen property (hue, saturation or
//! luminance); in black & white only luminance, which sets how light each colour turns grey.

use super::slider::{self, Range, SliderResponse};
use crate::theme;
use eframe::egui::{self, Color32, RichText};
use lf_settings::{EditSettings, HSL_BANDS, HslBand};

pub const BW_NOTE: &str =
    "In black & white, each colour's slider sets how light or dark it turns grey.";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Hue,
    Saturation,
    Luminance,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Hue, Mode::Saturation, Mode::Luminance];

    pub fn label(self) -> &'static str {
        match self {
            Mode::Hue => "Hue",
            Mode::Saturation => "Saturation",
            Mode::Luminance => "Luminance",
        }
    }
}

pub fn band_label(band: HslBand) -> &'static str {
    match band {
        HslBand::Red => "Red",
        HslBand::Orange => "Orange",
        HslBand::Yellow => "Yellow",
        HslBand::Green => "Green",
        HslBand::Aqua => "Aqua",
        HslBand::Blue => "Blue",
        HslBand::Purple => "Purple",
        HslBand::Magenta => "Magenta",
    }
}

fn band_colour(band: HslBand) -> Color32 {
    match band {
        HslBand::Red => Color32::from_rgb(0xef, 0x44, 0x44),
        HslBand::Orange => Color32::from_rgb(0xf9, 0x73, 0x16),
        HslBand::Yellow => Color32::from_rgb(0xea, 0xb3, 0x08),
        HslBand::Green => Color32::from_rgb(0x22, 0xc5, 0x5e),
        HslBand::Aqua => Color32::from_rgb(0x06, 0xb6, 0xd4),
        HslBand::Blue => Color32::from_rgb(0x3b, 0x82, 0xf6),
        HslBand::Purple => Color32::from_rgb(0x8b, 0x5c, 0xf6),
        HslBand::Magenta => Color32::from_rgb(0xd9, 0x46, 0xef),
    }
}

pub const TARGET: &str = "🎯 Drag in photo";

/// The section; `.1` is true when its targeted-adjustment button was clicked (`targeting`:
/// the tool is on).
pub fn show(
    ui: &mut egui::Ui,
    mode: &mut Mode,
    s: &mut EditSettings,
    targeting: bool,
) -> (SliderResponse, bool) {
    let target = ui
        .selectable_label(targeting, TARGET)
        .on_hover_text("Drag up or down on a colour in the photo to change its bands")
        .clicked();
    let shown = if s.bw {
        ui.label(RichText::new(BW_NOTE).small().color(theme::MUTED));
        Mode::Luminance
    } else {
        ui.horizontal(|ui| {
            for m in Mode::ALL {
                if ui.selectable_label(*mode == m, m.label()).clicked() {
                    *mode = m;
                }
            }
        });
        *mode
    };
    let mut out = SliderResponse::default();
    ui.push_id(shown.label(), |ui| {
        for band in HSL_BANDS {
            let adjust = &mut s.hsl[band];
            let value = match shown {
                Mode::Hue => &mut adjust.h,
                Mode::Saturation => &mut adjust.s,
                Mode::Luminance => &mut adjust.l,
            };
            ui.horizontal(|ui| {
                let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter()
                    .circle_filled(dot.center(), 5.0, band_colour(band));
                out = out.or(slider::edit(
                    ui,
                    band_label(band),
                    value,
                    Range::BIPOLAR,
                    None,
                ));
            });
        }
    });
    (out, target)
}
