//! "Versions": named snapshots of the photo's settings (label, date, a 112 px preview), with
//! "Add version…", "Use" (asks first; the current edit stays in history) and delete. The
//! version equal to the current settings is tagged "Current".

use super::presets::small_named;
use crate::theme;
use eframe::egui::{self, RichText};
use lf_catalog::Version;

pub const TITLE: &str = "Versions";
pub const ADD: &str = "Add version…";
pub const ADD_TITLE: &str = "Add version";
pub const LABEL: &str = "Label";
pub const SAVE: &str = "Add";
pub const USE: &str = "Use";
/// The question's button.
pub const USE_CONFIRM: &str = "Use version";
pub const USE_TITLE: &str = "Use this version?";
pub const USE_QUESTION: &str =
    "Replace the current edit with this version? Your current edit is kept in history.";
pub const DELETE: &str = "Delete";
pub const DELETE_CONFIRM: &str = "Delete version";
pub const DELETE_TEXT: &str = "The photo's current edit is not changed.";
pub const CURRENT: &str = "Current";
pub const EMPTY: &str = "Keep the edit as it is now under a name, to come back to it later.";
/// The preview's longest side.
pub const THUMB: u32 = 112;
/// The longest label.
pub const MAX_LABEL: usize = 80;

/// "Version 3" for the third.
pub fn default_label(existing: usize) -> String {
    format!("Version {}", existing + 1)
}

/// The delete question's title.
pub fn delete_title(label: &str) -> String {
    format!("Delete version “{label}”?")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionAction {
    Add,
    /// Ask to use the version with this id.
    Use(String),
    /// Ask to delete the version with this id.
    Delete(String),
}

/// One version as listed.
pub struct Row<'a> {
    pub version: &'a Version,
    /// The preview and its size in pixels.
    pub thumb: Option<(egui::TextureId, egui::Vec2)>,
    /// Its settings are the photo's current ones.
    pub current: bool,
}

/// The list; ids of the rows on screen without a preview yet are added to `wanted`.
pub fn show(ui: &mut egui::Ui, rows: &[Row], wanted: &mut Vec<String>) -> Option<VersionAction> {
    let mut action = None;
    if ui.button(ADD).clicked() {
        action = Some(VersionAction::Add);
    }
    if rows.is_empty() {
        ui.label(RichText::new(EMPTY).color(theme::MUTED).small());
        return action;
    }
    let side = THUMB as f32 / 2.0;
    egui::ScrollArea::vertical()
        .id_salt("versions-list")
        .max_height(4.0 * (side + 8.0))
        .show(ui, |ui| {
            for row in rows {
                let v = row.version;
                let inner = ui.horizontal(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
                    match row.thumb {
                        Some((texture, size)) => {
                            let k = (side / size.x.max(size.y).max(1.0)).min(1.0);
                            let shown = egui::Rect::from_center_size(rect.center(), size * k);
                            ui.painter().image(
                                texture,
                                shown,
                                egui::Rect::from_min_max(
                                    egui::pos2(0.0, 0.0),
                                    egui::pos2(1.0, 1.0),
                                ),
                                egui::Color32::WHITE,
                            );
                        }
                        None => {
                            ui.painter().rect_filled(rect, 4.0, theme::SURFACE_2);
                        }
                    }
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&v.label).strong());
                            if row.current {
                                ui.label(RichText::new(CURRENT).color(theme::SUCCESS).small());
                            }
                        });
                        ui.label(
                            RichText::new(v.created_at.format("%-d %b %Y, %H:%M").to_string())
                                .color(theme::MUTED)
                                .small(),
                        );
                        ui.horizontal(|ui| {
                            let name = format!("{USE} {}", v.label);
                            if small_named(ui, USE, &name).clicked() {
                                action = Some(VersionAction::Use(v.id.clone()));
                            }
                            let name = format!("{DELETE} {}", v.label);
                            if small_named(ui, DELETE, &name).clicked() {
                                action = Some(VersionAction::Delete(v.id.clone()));
                            }
                        });
                    });
                });
                if row.thumb.is_none() && ui.is_rect_visible(inner.response.rect) {
                    wanted.push(v.id.clone());
                }
            }
        });
    action
}
