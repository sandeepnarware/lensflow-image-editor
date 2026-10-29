//! Defringe (desktop only, `desktop.defringe`): purple and green fringes along high-contrast
//! edges, each with an amount and the hue it takes out, as Lightroom's. Shown for every photo
//! after Lens corrections; its "Reset" turns it off.

use super::slider::{self, Range, SliderResponse};
use eframe::egui;
use lf_settings::EditSettings;
use lf_settings::defringe::{self as d, Defringe};

pub const TITLE: &str = "Defringe";
pub const LABELS: [&str; 4] = ["Purple amount", "Purple hue", "Green amount", "Green hue"];

const AMOUNT: Range = Range::new(0.0, d::MAX_AMOUNT).step(1.0, 0);
const PURPLE: Range = Range::new(d::PURPLE_HUES.0, d::PURPLE_HUES.1).step(1.0, 0);
const GREEN: Range = Range::new(d::GREEN_HUES.0, d::GREEN_HUES.1).step(1.0, 0);

pub fn show(ui: &mut egui::Ui, s: &mut EditSettings) -> SliderResponse {
    let mut f = Defringe::from_settings(s);
    let mut out = SliderResponse::default();
    out = out.or(slider::edit(
        ui,
        LABELS[0],
        &mut f.purple_amount,
        AMOUNT,
        None,
    ));
    out = out.or(slider::edit(ui, LABELS[1], &mut f.purple_hue, PURPLE, None));
    out = out.or(slider::edit(
        ui,
        LABELS[2],
        &mut f.green_amount,
        AMOUNT,
        None,
    ));
    out = out.or(slider::edit(ui, LABELS[3], &mut f.green_hue, GREEN, None));
    if out.changed {
        f.write_into(s);
    }
    out
}

/// The section's "Reset": off.
pub fn reset(s: &mut EditSettings) {
    Defringe {
        purple_amount: 0.0,
        green_amount: 0.0,
        ..Defringe::default()
    }
    .write_into(s);
}
