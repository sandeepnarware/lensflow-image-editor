//! Which parts of an edit to copy, paste or keep in a preset, as the web's `GroupPicker.vue`:
//! "Include", All / None, and a checkbox per setting group (the 11 `GROUPS`). Also the
//! "Copy edits" dialog (`Ctrl+Shift+C`) built on it.

use crate::theme;
use eframe::egui::{self, RichText};
use lf_settings::groups::{DEFAULT_COPY_GROUPS, GROUPS, GroupKey};

pub const INCLUDE: &str = "Include";
pub const ALL: &str = "All";
pub const NONE: &str = "None";
pub const COPY_TITLE: &str = "Copy edits";
pub const COPY_TEXT: &str = "Paste them on another photo, or on many selected photos at once.";
pub const COPY: &str = "Copy";
pub const CANCEL: &str = "Cancel";

/// Turns `key` on or off, keeping `GROUPS` order.
pub fn toggle(groups: &mut Vec<GroupKey>, key: GroupKey, on: bool) {
    groups.retain(|g| *g != key);
    if on {
        groups.push(key);
        groups.sort_by_key(|g| GROUPS.iter().position(|(k, _, _)| k == g));
    }
}

/// The picker; `only` limits the boxes to those groups. True when the choice changed.
pub fn show(ui: &mut egui::Ui, groups: &mut Vec<GroupKey>, only: Option<&[GroupKey]>) -> bool {
    let keys: Vec<(GroupKey, &str)> = GROUPS
        .iter()
        .filter(|(k, _, _)| only.is_none_or(|o| o.contains(k)))
        .map(|(k, label, _)| (*k, *label))
        .collect();
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new(INCLUDE).strong().color(theme::INK));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.link(NONE).clicked() {
                groups.clear();
                changed = true;
            }
            if ui.link(ALL).clicked() {
                *groups = keys.iter().map(|(k, _)| *k).collect();
                changed = true;
            }
        });
    });
    egui::Grid::new(ui.scope_id().with("groups"))
        .num_columns(2)
        .show(ui, |ui| {
            for (i, (key, label)) in keys.iter().enumerate() {
                let mut on = groups.contains(key);
                if ui.checkbox(&mut on, *label).changed() {
                    toggle(groups, *key, on);
                    changed = true;
                }
                if i % 2 == 1 {
                    ui.end_row();
                }
            }
        });
    changed
}

/// What the copy dialog asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyAction {
    None,
    Copy(Vec<GroupKey>),
    Cancel,
}

/// The "Copy edits" dialog; the groups stay chosen between copies.
#[derive(Debug, Clone)]
pub struct CopyDialog {
    pub groups: Vec<GroupKey>,
}

impl Default for CopyDialog {
    fn default() -> Self {
        CopyDialog {
            groups: DEFAULT_COPY_GROUPS.to_vec(),
        }
    }
}

impl CopyDialog {
    pub fn show(&mut self, ctx: &egui::Context) -> CopyAction {
        let mut action = CopyAction::None;
        let _ = egui::Modal::new(egui::Id::unique("copy-edits")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.heading(COPY_TITLE);
            ui.label(RichText::new(COPY_TEXT).color(theme::MUTED));
            ui.add_space(8.0);
            show(ui, &mut self.groups, None);
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!self.groups.is_empty(), egui::Button::new(COPY))
                    .clicked()
                {
                    action = CopyAction::Copy(self.groups.clone());
                }
                if ui.button(CANCEL).clicked() {
                    action = CopyAction::Cancel;
                }
            });
        });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && action == CopyAction::None {
            action = CopyAction::Cancel;
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggling_keeps_the_groups_in_order() {
        let mut g = vec![GroupKey::Bw];
        toggle(&mut g, GroupKey::Light, true);
        toggle(&mut g, GroupKey::Spots, true);
        assert_eq!(g, [GroupKey::Light, GroupKey::Bw, GroupKey::Spots]);
        toggle(&mut g, GroupKey::Bw, false);
        assert_eq!(g, [GroupKey::Light, GroupKey::Spots]);
        assert_eq!(CopyDialog::default().groups, DEFAULT_COPY_GROUPS);
    }
}
