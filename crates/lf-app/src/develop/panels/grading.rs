//! Colour grading: a hue and an amount for shadows, midtones and highlights, and the balance
//! between them.

use super::slider::{self, Range, SliderResponse, Track};
use eframe::egui::{self, RichText};
use lf_settings::{EditSettings, GradeWheel};

/// A wheel's hue: 0..360 on a rainbow track, back to the wheel's default hue on double-click.
pub fn hue_range(neutral: f64) -> Range {
    Range::new(0.0, 360.0).neutral(neutral).unit("°")
}

fn wheel(
    ui: &mut egui::Ui,
    label: &str,
    w: &mut GradeWheel,
    default: &GradeWheel,
) -> SliderResponse {
    ui.push_id(label, |ui| {
        ui.horizontal(|ui| {
            let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            let colour = egui::ecolor::Hsva::new(
                (w.hue / 360.0) as f32,
                (w.sat.max(15.0) / 100.0) as f32,
                0.8,
                1.0,
            );
            ui.painter().circle_filled(dot.center(), 5.0, colour);
            ui.label(RichText::new(label).small().strong());
        });
        let mut out = slider::edit(
            ui,
            "Hue",
            &mut w.hue,
            hue_range(default.hue),
            Some(Track::Rainbow),
        );
        out = out.or(slider::edit(ui, "Amount", &mut w.sat, Range::AMOUNT, None));
        out
    })
    .inner
}

pub fn show(ui: &mut egui::Ui, s: &mut EditSettings) -> SliderResponse {
    let d = EditSettings::default_settings().grading;
    let g = &mut s.grading;
    let mut out = wheel(ui, "Shadows", &mut g.shadows, &d.shadows);
    out = out.or(wheel(ui, "Midtones", &mut g.midtones, &d.midtones));
    out = out.or(wheel(ui, "Highlights", &mut g.highlights, &d.highlights));
    out.or(slider::edit(
        ui,
        "Balance",
        &mut g.balance,
        Range::BIPOLAR,
        None,
    ))
}
