//! Library → Tag from GPX track…: the track file, the camera clock's time zone, and whether
//! photos that already have a place keep it. The work is [`lf_catalog::gpx`] on the `Io` lane
//! (`library/geotag_ui.rs`).

use crate::theme;
use eframe::egui::{self, RichText};
use std::path::PathBuf;

pub const TITLE: &str = "Tag from GPX track";
pub const TEXT: &str = "Each photo is placed where the track was when it was taken. Cameras keep local time: choose the time zone the camera's clock was set to.";
pub const FILE: &str = "Track";
pub const ZONE: &str = "Camera time zone";
pub const KEEP: &str = "Only photos without a place";
pub const CHOOSE: &str = "Choose…";
pub const TAG: &str = "Tag photos";
pub const CANCEL: &str = "Cancel";

#[derive(Debug, Clone, PartialEq)]
pub enum GeotagAction {
    None,
    Choose,
    Tag {
        file: PathBuf,
        offset_minutes: i32,
        only_missing: bool,
    },
    Close,
}

pub struct GeotagDialog {
    pub ids: Vec<i64>,
    pub file: Option<PathBuf>,
    pub offset_minutes: i32,
    pub only_missing: bool,
    pub ticket: u64,
}

/// `+05:30`, `-08:00`.
pub fn zone_name(minutes: i32) -> String {
    let sign = if minutes < 0 { '-' } else { '+' };
    let m = minutes.abs();
    format!("UTC{sign}{:02}:{:02}", m / 60, m % 60)
}

impl GeotagDialog {
    /// For `ids`, the camera's zone guessed as this computer's.
    pub fn new(ids: Vec<i64>) -> GeotagDialog {
        let here = chrono::Local::now().offset().local_minus_utc() / 60;
        GeotagDialog {
            ids,
            file: None,
            offset_minutes: here,
            only_missing: true,
            ticket: 0,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> GeotagAction {
        let mut action = GeotagAction::None;
        let _ = egui::Modal::new(egui::Id::unique("geotag")).show(ctx, |ui| {
            ui.set_max_width(440.0);
            ui.heading(TITLE);
            ui.label(RichText::new(TEXT).color(theme::MUTED));
            ui.add_space(8.0);
            egui::Grid::new("geotag-grid")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label(FILE);
                    ui.horizontal(|ui| {
                        let name = self
                            .file
                            .as_ref()
                            .and_then(|f| f.file_name())
                            .map_or("None chosen".to_owned(), |n| {
                                n.to_string_lossy().into_owned()
                            });
                        ui.label(name);
                        if ui.button(CHOOSE).clicked() {
                            action = GeotagAction::Choose;
                        }
                    });
                    ui.end_row();
                    ui.label(ZONE);
                    let combo = egui::ComboBox::from_id_salt("geotag-zone")
                        .selected_text(zone_name(self.offset_minutes))
                        .show_ui(ui, |ui| {
                            for m in (-12 * 60..=14 * 60).step_by(30) {
                                ui.selectable_value(&mut self.offset_minutes, m, zone_name(m));
                            }
                        });
                    combo.response.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::ComboBox, true, ZONE)
                    });
                    ui.end_row();
                });
            ui.checkbox(&mut self.only_missing, KEEP);
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(self.file.is_some(), egui::Button::new(TAG))
                    .clicked()
                    && let Some(file) = &self.file
                {
                    action = GeotagAction::Tag {
                        file: file.clone(),
                        offset_minutes: self.offset_minutes,
                        only_missing: self.only_missing,
                    };
                }
                if ui.button(CANCEL).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    action = GeotagAction::Close;
                }
            });
        });
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zones_read_as_utc_offsets() {
        assert_eq!(zone_name(330), "UTC+05:30");
        assert_eq!(zone_name(-480), "UTC-08:00");
        assert_eq!(zone_name(0), "UTC+00:00");
    }
}
