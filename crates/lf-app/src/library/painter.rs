//! The Painter (Lightroom's spray can): with it on, a click on a thumbnail (or a drag across
//! several) gives each one what is chosen in the painter's bar over the grid: a keyword, a
//! star rating, a colour label or a flag. Marks are set as chosen (never toggled off), each
//! photo once a stroke; the selection is left alone.

use super::{Env, Library};
use crate::theme;
use eframe::egui::{self, RichText};
use lf_catalog::cull::{Flag, Label, Mark};
use std::collections::HashSet;

pub const PAINT: &str = "Paint";
pub const KEYWORD_HINT: &str = "Keyword to paint";
pub const DONE: &str = "Done";
pub const HINT: &str = "Click or drag across photos to paint them.";

/// What the painter gives.
#[derive(Debug, Clone, PartialEq)]
pub enum Paint {
    Keyword(String),
    Mark(Mark),
}

impl Paint {
    /// The choices of the bar's list (a keyword is typed beside it).
    pub fn choices() -> Vec<(Paint, String)> {
        let mut v = vec![(Paint::Keyword(String::new()), "Keyword".to_owned())];
        for stars in 0..=5u8 {
            v.push((Paint::Mark(Mark::Rating(stars)), format!("{stars} stars")));
        }
        for (flag, name) in [
            (Flag::Picked, "Pick"),
            (Flag::Rejected, "Reject"),
            (Flag::Unflagged, "Unflag"),
        ] {
            v.push((Paint::Mark(Mark::Flag(flag)), name.to_owned()));
        }
        for label in Label::ALL {
            v.push((
                Paint::Mark(Mark::Label(Some(label))),
                format!("{label:?} label"),
            ));
        }
        v
    }

    fn same_kind(&self, other: &Paint) -> bool {
        match (self, other) {
            (Paint::Keyword(_), Paint::Keyword(_)) => true,
            (a, b) => a == b,
        }
    }
}

/// The painter while on: what it gives, and the photos painted in the stroke under way.
#[derive(Debug, Clone)]
pub struct Painter {
    pub paint: Paint,
    stroke: HashSet<i64>,
    /// Painted this frame, to apply once the grid is drawn.
    pub queue: Vec<i64>,
}

impl Painter {
    pub fn new() -> Painter {
        Painter {
            paint: Paint::Keyword(String::new()),
            stroke: HashSet::new(),
            queue: Vec::new(),
        }
    }

    /// The pointer is on `id` with the button down: it is painted, once a stroke.
    pub fn touch(&mut self, id: i64) {
        if self.stroke.insert(id) {
            self.queue.push(id);
        }
    }

    /// The button came up: the next stroke may paint the same photos again.
    pub fn lift(&mut self) {
        self.stroke.clear();
    }
}

impl Default for Painter {
    fn default() -> Self {
        Painter::new()
    }
}

impl Library {
    pub fn painting(&self) -> bool {
        self.painter.is_some()
    }

    pub(super) fn toggle_painter(&mut self) {
        self.painter = match self.painter {
            Some(_) => None,
            None => Some(Painter::new()),
        };
    }

    /// The bar over the grid while the painter is on.
    pub(super) fn painter_bar(&mut self, ui: &mut egui::Ui) {
        let Some(p) = &mut self.painter else {
            return;
        };
        let mut done = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new(PAINT).strong());
            let current = Paint::choices()
                .into_iter()
                .find(|(c, _)| c.same_kind(&p.paint))
                .map(|(_, n)| n)
                .unwrap_or_default();
            let combo = egui::ComboBox::from_id_salt("painter-kind")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (choice, name) in Paint::choices() {
                        if ui
                            .selectable_label(choice.same_kind(&p.paint), name)
                            .clicked()
                            && !choice.same_kind(&p.paint)
                        {
                            p.paint = choice;
                        }
                    }
                });
            combo
                .response
                .widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, PAINT));
            if let Paint::Keyword(k) = &mut p.paint {
                let field = ui.add(
                    egui::TextEdit::singleline(k)
                        .hint_text(KEYWORD_HINT)
                        .desired_width(160.0),
                );
                field.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::TextInput, true, KEYWORD_HINT)
                });
            }
            ui.label(RichText::new(HINT).color(theme::MUTED).small());
            done = ui.button(DONE).clicked();
        });
        if done {
            self.painter = None;
        }
    }

    /// Applies what the grid painted this frame.
    pub(super) fn apply_paint(&mut self, ui: &egui::Ui, env: &mut Env) {
        let Some(p) = &mut self.painter else {
            return;
        };
        if !ui.input(|i| i.pointer.primary_down()) {
            p.lift();
        }
        let ids = std::mem::take(&mut p.queue);
        if ids.is_empty() {
            return;
        }
        match p.paint.clone() {
            Paint::Keyword(k) => {
                let words = lf_catalog::meta::parse_keywords(&k);
                if !words.is_empty() {
                    self.change_meta(ids, lf_catalog::meta::Change::AddKeywords(words), env);
                }
            }
            Paint::Mark(mark) => self.set_mark(ids, mark, env),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stroke_paints_each_photo_once() {
        let mut p = Painter::new();
        p.touch(1);
        p.touch(1);
        p.touch(2);
        assert_eq!(p.queue, vec![1, 2]);
        p.queue.clear();
        p.lift();
        p.touch(1);
        assert_eq!(p.queue, vec![1], "a new stroke");
        assert!(Paint::choices().len() > 10);
    }
}
