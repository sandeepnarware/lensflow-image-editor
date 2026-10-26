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

/// The panel for `s` on a `width`×`height` photo.
pub fn show(ui: &mut egui::Ui, s: &mut EditSettings, width: u32, height: u32) -> CropPanelOutput {
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

    if ui.link(RESET).clicked() {
        tool::reset(s, tw, th);
        out.changed = true;
    }
    out
}
