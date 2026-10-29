//! Develop's History section (as Lightroom's History panel): the open photo's edit steps,
//! newest first, each named by what it changed ("Exposure", "Crop", "Masks +2"), the one the
//! photo stands at marked; a click goes back (or forward) to it, as several undos would. The
//! steps are read from the catalogue again only when the photo, its committed settings or its
//! undo position change.

use super::Develop;
use crate::theme;
use crate::toast::Toasts;
use eframe::egui::{self, RichText};
use lf_settings::EditSettings;
use serde_json::Value;

pub const TITLE: &str = "History";
pub const FIRST: &str = "Opened";
pub const NONE: &str = "No edits yet.";

/// A key's name as people read it: `colorNoise` → "Color noise", `pointColour` → "Point colour".
fn readable(key: &str) -> String {
    let mut out = String::new();
    for (i, c) in key.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push(' ');
            out.extend(c.to_lowercase());
        } else if i == 0 {
            out.extend(c.to_uppercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// What changed from `before` to `after`, as one short name: the first changed setting (a
/// desktop-only one by its own key), and how many more ("Exposure +2").
pub fn describe(before: &EditSettings, after: &EditSettings) -> String {
    let (b, a) = (before.to_value(), after.to_value());
    let empty = serde_json::Map::new();
    let (b, a) = (
        b.as_object().unwrap_or(&empty),
        a.as_object().unwrap_or(&empty),
    );
    let mut keys: Vec<String> = Vec::new();
    let mut all: Vec<&String> = b.keys().chain(a.keys()).collect();
    all.sort();
    all.dedup();
    for k in all {
        if k == "updatedAt" || b.get(k) == a.get(k) {
            continue;
        }
        if k == "desktop" {
            let bd = b.get(k).and_then(Value::as_object).unwrap_or(&empty);
            let ad = a.get(k).and_then(Value::as_object).unwrap_or(&empty);
            let mut inner: Vec<&String> = bd.keys().chain(ad.keys()).collect();
            inner.sort();
            inner.dedup();
            keys.extend(
                inner
                    .into_iter()
                    .filter(|d| bd.get(*d) != ad.get(*d))
                    .cloned(),
            );
        } else {
            keys.push(k.clone());
        }
    }
    match keys.len() {
        0 => "No change".to_owned(),
        1 => readable(&keys[0]),
        n => format!("{} +{}", readable(&keys[0]), n - 1),
    }
}

/// The History section's rows, read when stale.
#[derive(Default)]
pub struct HistoryUi {
    /// (photo, undo position, committed settings) the rows were read for.
    key: Option<(i64, (i64, i64), EditSettings)>,
    /// (step id, name), newest first; and the step the photo stands at.
    rows: Vec<(i64, String)>,
    cursor: Option<i64>,
}

impl Develop {
    pub(super) fn history_ui(&mut self, ui: &mut egui::Ui, toasts: &mut Toasts) {
        egui::CollapsingHeader::new(TITLE)
            .id_salt("history-section")
            .show(ui, |ui| self.history_body(ui, toasts));
    }

    fn history_body(&mut self, ui: &mut egui::Ui, toasts: &mut Toasts) {
        let Some(edit) = self.session.edit() else {
            return;
        };
        let key = (edit.photo_id, edit.steps(), edit.committed.clone());
        if self.history.key.as_ref() != Some(&key) {
            match self
                .session
                .catalog()
                .and_then(|c| lf_catalog::edits::history(c, key.0).map_err(|e| e.to_string()))
            {
                Ok((entries, cursor)) => {
                    let mut rows: Vec<(i64, String)> = entries
                        .iter()
                        .enumerate()
                        .map(|(i, e)| {
                            let name = match i {
                                0 => FIRST.to_owned(),
                                _ => describe(&entries[i - 1].settings, &e.settings),
                            };
                            (e.id, name)
                        })
                        .collect();
                    rows.reverse();
                    self.history.rows = rows;
                    self.history.cursor = cursor;
                }
                Err(e) => log::warn!("could not read the history: {e}"),
            }
            self.history.key = Some(key);
        }
        if self.history.rows.is_empty() {
            ui.label(RichText::new(NONE).color(theme::MUTED));
            return;
        }
        let mut go = None;
        egui::ScrollArea::vertical()
            .id_salt("history-rows")
            .max_height(220.0)
            .show(ui, |ui| {
                for (id, name) in &self.history.rows {
                    let here = self.history.cursor == Some(*id);
                    if ui.selectable_label(here, name).clicked() && !here {
                        go = Some(*id);
                    }
                }
            });
        if let Some(id) = go {
            self.jump_history(id, toasts);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_is_named_by_what_it_changed() {
        let a = EditSettings::default_settings();
        let mut b = a.clone();
        b.exposure = 1.0;
        assert_eq!(describe(&a, &b), "Exposure");
        b.contrast = 10.0;
        b.saturation = 5.0;
        assert_eq!(describe(&a, &b), "Contrast +2");
        let mut c = a.clone();
        lf_settings::defringe::Defringe {
            purple_amount: 5.0,
            ..Default::default()
        }
        .write_into(&mut c);
        assert_eq!(describe(&a, &c), "Defringe");
        assert_eq!(readable("colorNoise"), "Color noise");
        assert_eq!(describe(&a, &a), "No change");
    }
}
