//! The crop tool's panel, as the web's `CropPanel.vue`: aspect ratios (with "Switch portrait /
//! landscape" for a locked shape), rotate and flip, Straighten, and "Reset crop & rotation".
//!
//! The aspects are the web's full list. The aspect and the reset are part of the crop session (committed on `Enter` or when the
//! tool is left); a turn or a flip commits at once, and Straighten commits on release.

use super::slider::{self, Range};
use crate::develop::tools::crop::{self as tool, ASPECTS, RESET, STRAIGHTEN, SWITCH};
use crate::theme;
use eframe::egui::{self, RichText};
use lf_settings::EditSettings;
use lf_settings::geometry::TurnDir;

/// Straighten: −45..45 in tenths of a degree.
pub const STRAIGHTEN_RANGE: Range = Range::new(-45.0, 45.0).step(0.1, 1).unit("°");
pub const ASPECT_RATIO: &str = "Aspect ratio";
pub const ROTATE_FLIP: &str = "Rotate & flip";
pub const ROTATE_LEFT: &str = "Rotate left";
pub const ROTATE_RIGHT: &str = "Rotate right";
pub const FLIP_H: &str = "Flip horizontally";
pub const FLIP_V: &str = "Flip vertically";
pub const AUTO_STRAIGHTEN: &str = "Auto straighten";
pub const NO_LEVEL: &str = "Not enough straight lines were found to level the photo.";

/// What the panel did this frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CropPanelOutput {
    /// The settings changed (redraw).
    pub changed: bool,
    /// Commit now (a turn, a flip, the end of a straighten drag).
    pub commit: bool,
    /// The straighten drag started (`Some(true)`) or ended (`Some(false)`).
    pub straightening: Option<bool>,
}

/// The straightening angle that levels the photo's lines (Upright's Level), set on `s`; false
/// when too few lines were found (or the photo is turned or flipped, which the fit cannot
/// read).
pub fn auto_straighten(
    s: &mut EditSettings,
    sample: &crate::develop::auto::Sample,
    width: u32,
    height: u32,
) -> bool {
    if s.rotate != lf_settings::Rotate::R0 || s.flip_h || s.flip_v {
        return false;
    }
    let segments = crate::develop::upright::segments(sample);
    let aspect = f64::from(sample.width) / f64::from(sample.height.max(1));
    let lens = lf_settings::lens::Lens::from_settings(s);
    let Some(fit) =
        lf_settings::upright::fit(&segments, aspect, &lens, lf_settings::upright::Mode::Level)
    else {
        return false;
    };
    let (tw, th) = tool::turned(s, width, height);
    tool::straighten(s, fit.angle, tw, th);
    true
}

/// The panel for `s` on a `width`×`height` photo; `sample` (the photo, small) for Auto
/// straighten.
pub fn show(
    ui: &mut egui::Ui,
    s: &mut EditSettings,
    width: u32,
    height: u32,
    sample: Option<&crate::develop::auto::Sample>,
) -> CropPanelOutput {
    let mut out = CropPanelOutput::default();
    let (tw, th) = tool::turned(s, width, height);

    ui.label(RichText::new(ASPECT_RATIO).strong().color(theme::INK));
    ui.horizontal_wrapped(|ui| {
        for (value, label) in ASPECTS {
            // Clicking the chosen shape again fits the frame to it again, as on the web.
            let on = tool::aspect_lit(value, &s.aspect);
            if ui.selectable_label(on, label).clicked() {
                tool::set_aspect(s, value, tw, th);
                out.changed = true;
            }
        }
    });
    if tool::flippable(&s.aspect) && ui.link(SWITCH).clicked() {
        let other = tool::swapped(&s.aspect);
        tool::set_aspect(s, &other, tw, th);
        out.changed = true;
    }
    ui.add_space(6.0);

    ui.label(RichText::new(ROTATE_FLIP).strong().color(theme::INK));
    egui::Grid::new("crop-rotate-flip")
        .num_columns(2)
        .show(ui, |ui| {
            if ui.button(ROTATE_LEFT).clicked() {
                tool::turn(s, TurnDir::Anticlockwise);
                out.changed = true;
                out.commit = true;
            }
            if ui.button(ROTATE_RIGHT).clicked() {
                tool::turn(s, TurnDir::Clockwise);
                out.changed = true;
                out.commit = true;
            }
            ui.end_row();
            if ui.selectable_label(s.flip_h, FLIP_H).clicked() {
                tool::flip(s, true);
                out.changed = true;
                out.commit = true;
            }
            if ui.selectable_label(s.flip_v, FLIP_V).clicked() {
                tool::flip(s, false);
                out.changed = true;
                out.commit = true;
            }
            ui.end_row();
        });
    ui.add_space(6.0);

    // A turn above changed the frame's sides.
    let (tw, th) = tool::turned(s, width, height);
    let mut angle = s.angle;
    let r = slider::edit(ui, STRAIGHTEN, &mut angle, STRAIGHTEN_RANGE, None);
    if r.changed {
        tool::straighten(s, angle, tw, th);
        out.changed = true;
        out.straightening = Some(true);
    }
    if r.released {
        out.commit = true;
        out.straightening = Some(false);
    }
    let note = egui::Id::unique("auto-straighten-note");
    if ui
        .add_enabled(sample.is_some(), egui::Button::new(AUTO_STRAIGHTEN))
        .on_hover_text("Level the photo by its lines")
        .clicked()
        && let Some(sample) = sample
    {
        let done = auto_straighten(s, sample, width, height);
        ui.ctx().data_mut(|d| d.insert_temp(note, !done));
        if done {
            out.changed = true;
            out.commit = true;
        }
    }
    if ui.ctx().data(|d| d.get_temp::<bool>(note)).unwrap_or(false) {
        ui.label(RichText::new(NO_LEVEL).color(theme::MUTED));
    }

    if ui.link(RESET).clicked() {
        tool::reset(s, tw, th);
        out.changed = true;
    }
    out
}

#[cfg(test)]
mod auto_tests {
    use super::*;
    use crate::develop::auto::Sample;

    /// A light sky over dark ground, the horizon tilted `slope` (down to the right), and two
    /// more dark lines parallel to it.
    fn tilted(slope: f32) -> Sample {
        let (w, h) = (240u32, 160u32);
        let mut rgb = vec![[0.85f32; 3]; (w * h) as usize];
        for y in 0..h {
            for x in 0..w {
                let tilt = x as f32 * slope;
                for base in [50.0f32, 80.0, 110.0] {
                    if (y as f32 - (base + tilt)).abs() < 3.0 {
                        rgb[(y * w + x) as usize] = [0.1; 3];
                    }
                }
            }
        }
        Sample {
            width: w,
            height: h,
            rgb,
        }
    }

    #[test]
    fn auto_straighten_levels_a_tilted_horizon() {
        let sample = tilted(0.05);
        let mut s = EditSettings::default_settings();
        assert!(auto_straighten(&mut s, &sample, 240, 160));
        assert!(
            (1.5..=4.5).contains(&s.angle.abs()),
            "about 2.9 degrees: {}",
            s.angle
        );
        // A turned photo is left alone.
        let mut turned = EditSettings::default_settings();
        turned.rotate = lf_settings::Rotate::R90;
        assert!(!auto_straighten(&mut turned, &sample, 240, 160));
        assert_eq!(turned.angle, 0.0);
    }
}
