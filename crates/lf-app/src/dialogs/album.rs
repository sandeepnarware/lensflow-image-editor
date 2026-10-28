//! New album, New smart album and Edit album: a name, and for a smart album the rule that
//! picks its photos (stars, flag, colour label, words, how recent). A new plain album can
//! take the selected photos at once. Saving is the library's ([`crate::library`]), on the
//! `Io` lane; a refused name comes back here as `error`.

use crate::theme;
use eframe::egui::{self, Key, RichText};
use lf_catalog::albums::{Rule, RuleFlag};
use lf_catalog::cull::Label;

pub const NAME: &str = "Album name";
pub const SAVE: &str = "Save";
pub const CREATE: &str = "Create";
pub const CANCEL: &str = "Cancel";
pub const STARS: &str = "At least";
pub const FLAG: &str = "Flag";
pub const LABEL: &str = "Colour label";
pub const WORDS: &str = "Words";
pub const WITHIN: &str = "Taken within";
pub const ABOUT_SMART: &str = "A smart album holds every photo in the catalogue that matches all of these, and keeps up as you rate, flag and label.";

/// What the dialog was opened for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    New,
    NewSmart,
    Edit(i64),
}

/// What the dialog asks of the library this frame.
#[derive(Debug, Clone, PartialEq)]
pub enum AlbumAction {
    None,
    /// Save `name` (and the rule, for a smart album); `with` photos go in a new plain album.
    Save {
        purpose: Purpose,
        name: String,
        rule: Option<Rule>,
        with: Vec<i64>,
    },
    Close,
}

/// The "Taken within" choices, in days (0: any time).
pub const WITHIN_DAYS: [(u32, &str); 6] = [
    (0, "Any time"),
    (1, "The last day"),
    (7, "The last week"),
    (30, "The last 30 days"),
    (90, "The last 90 days"),
    (365, "The last year"),
];

pub struct AlbumDialog {
    pub purpose: Purpose,
    pub name: String,
    /// Set for a smart album.
    pub rule: Option<Rule>,
    /// The selected photos, offered to a new plain album.
    pub selected: Vec<i64>,
    pub include_selected: bool,
    /// Why the last save was refused.
    pub error: Option<String>,
    /// A save is on its way; the buttons wait.
    pub saving: bool,
}

impl AlbumDialog {
    pub fn new(purpose: Purpose, selected: Vec<i64>) -> AlbumDialog {
        AlbumDialog {
            purpose,
            name: String::new(),
            rule: (purpose == Purpose::NewSmart).then(Rule::default),
            include_selected: !selected.is_empty(),
            selected,
            error: None,
            saving: false,
        }
    }

    /// Edit an album that exists.
    pub fn edit(id: i64, name: &str, rule: Option<Rule>) -> AlbumDialog {
        AlbumDialog {
            purpose: Purpose::Edit(id),
            name: name.to_owned(),
            rule,
            selected: Vec::new(),
            include_selected: false,
            error: None,
            saving: false,
        }
    }

    pub fn title(&self) -> &'static str {
        match (self.purpose, self.rule.is_some()) {
            (Purpose::New, _) => "New album",
            (Purpose::NewSmart, _) => "New smart album",
            (Purpose::Edit(_), true) => "Edit smart album",
            (Purpose::Edit(_), false) => "Rename album",
        }
    }

    /// The save this dialog would ask for now.
    fn save(&self) -> AlbumAction {
        let with = if self.purpose == Purpose::New && self.include_selected {
            self.selected.clone()
        } else {
            Vec::new()
        };
        AlbumAction::Save {
            purpose: self.purpose,
            name: self.name.trim().to_owned(),
            rule: self.rule.clone(),
            with,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> AlbumAction {
        let mut action = AlbumAction::None;
        let _ = egui::Modal::new(egui::Id::unique("album-dialog")).show(ctx, |ui| {
            ui.set_width(400.0);
            ui.heading(self.title());
            ui.add_space(8.0);
            let name = ui.add(
                egui::TextEdit::singleline(&mut self.name)
                    .hint_text("Client picks")
                    .desired_width(f32::INFINITY),
            );
            name.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, NAME));
            if self.name.is_empty() && !name.has_focus() && !name.lost_focus() {
                name.request_focus();
            }
            let enter = name.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            if let Some(rule) = &mut self.rule {
                ui.add_space(8.0);
                rule_ui(ui, rule);
                ui.label(RichText::new(ABOUT_SMART).color(theme::MUTED));
            }
            if self.purpose == Purpose::New && !self.selected.is_empty() {
                ui.add_space(4.0);
                let n = self.selected.len();
                ui.checkbox(
                    &mut self.include_selected,
                    if n == 1 {
                        "Add the selected photo".to_owned()
                    } else {
                        format!("Add the {n} selected photos")
                    },
                );
            }
            if let Some(e) = &self.error {
                ui.label(RichText::new(e).color(theme::WARNING));
            }
            ui.add_space(12.0);
            let ready = !self.name.trim().is_empty() && !self.saving;
            ui.horizontal(|ui| {
                let label = if matches!(self.purpose, Purpose::Edit(_)) {
                    SAVE
                } else {
                    CREATE
                };
                if (ui.add_enabled(ready, egui::Button::new(label)).clicked() || (enter && ready))
                    && action == AlbumAction::None
                {
                    action = self.save();
                }
                if ui.button(CANCEL).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                    action = AlbumAction::Close;
                }
            });
        });
        if matches!(action, AlbumAction::Save { .. }) {
            self.saving = true;
            self.error = None;
        }
        action
    }
}

/// The smart album's rule, as a form.
fn rule_ui(ui: &mut egui::Ui, rule: &mut Rule) {
    egui::Grid::new("album-rule")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label(STARS);
            let stars = |n: u8| match n {
                0 => "Any rating".to_owned(),
                1 => "1 star".to_owned(),
                n => format!("{n} stars"),
            };
            let r = egui::ComboBox::from_id_salt("album-stars")
                .selected_text(stars(rule.min_rating.min(5)))
                .show_ui(ui, |ui| {
                    for n in 0..=5 {
                        ui.selectable_value(&mut rule.min_rating, n, stars(n));
                    }
                });
            named(&r.response, STARS);
            ui.end_row();

            ui.label(FLAG);
            let r = egui::ComboBox::from_id_salt("album-flag")
                .selected_text(rule.flag.name())
                .show_ui(ui, |ui| {
                    for f in RuleFlag::ALL {
                        ui.selectable_value(&mut rule.flag, f, f.name());
                    }
                });
            named(&r.response, FLAG);
            ui.end_row();

            ui.label(LABEL);
            let current = rule
                .label
                .as_deref()
                .and_then(Label::from_key)
                .map_or("Any label", Label::name);
            let r = egui::ComboBox::from_id_salt("album-label")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut rule.label, None, "Any label");
                    for l in Label::ALL {
                        ui.selectable_value(&mut rule.label, Some(l.key().to_owned()), l.name());
                    }
                });
            named(&r.response, LABEL);
            ui.end_row();

            ui.label(WORDS);
            let r = ui.add(
                egui::TextEdit::singleline(&mut rule.text)
                    .hint_text("wedding Sharma")
                    .desired_width(200.0),
            );
            r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, WORDS));
            ui.end_row();

            ui.label(WITHIN);
            let current = WITHIN_DAYS
                .iter()
                .find(|(d, _)| *d == rule.within_days)
                .map_or("Any time", |(_, n)| n);
            let r = egui::ComboBox::from_id_salt("album-within")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (days, name) in WITHIN_DAYS {
                        ui.selectable_value(&mut rule.within_days, days, name);
                    }
                });
            named(&r.response, WITHIN);
            ui.end_row();
        });
}

fn named(response: &egui::Response, name: &str) {
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, name));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_album_takes_the_selection_unless_told_not_to() {
        let mut d = AlbumDialog::new(Purpose::New, vec![3, 4]);
        d.name = "  Picks ".into();
        assert_eq!(
            d.save(),
            AlbumAction::Save {
                purpose: Purpose::New,
                name: "Picks".into(),
                rule: None,
                with: vec![3, 4],
            }
        );
        d.include_selected = false;
        assert!(matches!(d.save(), AlbumAction::Save { with, .. } if with.is_empty()));
        let smart = AlbumDialog::new(Purpose::NewSmart, vec![3]);
        assert_eq!(smart.title(), "New smart album");
        assert!(
            matches!(smart.save(), AlbumAction::Save { rule: Some(_), with, .. } if with.is_empty())
        );
    }
}
