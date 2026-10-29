//! The Library's Metadata panel (right side, toggled from the ribbon): the selected photos'
//! title, caption, creator and copyright, and their keywords, with the folder's keywords
//! one click away. Like a flag or a star, a change shows at once (the rows held in memory
//! are changed) and is saved on the `Io` lane ([`lf_catalog::meta::Change`]); a failed save
//! reloads the folder's rows.
//!
//! A text field is saved when it loses the keyboard (or on Enter, for the one-line fields),
//! and only when it changed. With several photos selected a field they disagree on shows
//! empty with "Several values", and is left alone unless something is typed into it.

use super::{Env, Library, Mode};
use crate::catalog_ext;
use crate::jobs::{Lane, Msg};
use crate::theme;
use crate::toast::Level;
use eframe::egui::{self, RichText};
use lf_catalog::meta::{Change, Field, parse_keywords};

/// The panel's width, in points.
pub const WIDTH: f32 = 260.0;
pub const TITLE: &str = "Metadata";
pub const NOTHING_SELECTED: &str = "Select photos to describe them.";
pub const SEVERAL: &str = "Several values";
pub const ADD_KEYWORDS: &str = "Add keywords";
pub const ADD_HINT: &str = "Add keywords, separated by commas";
pub const IN_FOLDER: &str = "Keywords in this folder";
pub const SUGGESTED: &str = "Suggested by AI";
pub const SAVE_FAILED: &str = "Could not save the metadata.";
pub const PRESET: &str = "Metadata preset";
pub const APPLY_PRESET: &str = "Apply a preset…";
pub const SAVE_PRESET: &str = "Save as preset";
pub const PRESET_NAME: &str = "Preset name";
pub const PRESET_HINT: &str = "The creator, copyright and keywords of the first selected photo";
pub const SYNC: &str = "Sync from active photo";
pub const SYNC_HINT: &str = "Give the other selected photos the active photo's title, caption, creator, copyright and keywords";

/// The changes that give other photos `from`'s metadata: every field it has set, and its
/// keywords added.
pub fn sync_changes(from: &lf_catalog::meta::Meta) -> Vec<Change> {
    let mut out: Vec<Change> = Field::ALL
        .into_iter()
        .filter_map(|f| f.of(from).map(|v| Change::Set(f, v.to_owned())))
        .collect();
    if !from.keywords.is_empty() {
        out.push(Change::AddKeywords(from.keywords.clone()));
    }
    out
}

/// The changes a metadata preset makes: its creator and copyright (when it has them) set, its
/// keywords added.
pub fn preset_changes(p: &crate::prefs::MetaPreset) -> Vec<Change> {
    let mut out = Vec::new();
    if let Some(c) = p.creator.as_ref().filter(|c| !c.trim().is_empty()) {
        out.push(Change::Set(Field::Creator, c.clone()));
    }
    if let Some(c) = p.copyright.as_ref().filter(|c| !c.trim().is_empty()) {
        out.push(Change::Set(Field::Copyright, c.clone()));
    }
    if !p.keywords.is_empty() {
        out.push(Change::AddKeywords(p.keywords.clone()));
    }
    out
}

/// What the panel keeps between frames: the text being typed, for the photos it was
/// started on.
#[derive(Debug, Default)]
pub struct MetaPanel {
    pub shown: bool,
    /// The selection the drafts belong to; a new selection starts them again.
    ids: Vec<i64>,
    /// Each field's text as typed, and the text it started from.
    drafts: Vec<(Field, String, String)>,
    adding: String,
    /// A preset's name being typed (Save as preset).
    preset_name: Option<String>,
}

impl Library {
    /// Shows or hides the Metadata panel (Library only).
    pub fn toggle_metadata(&mut self) {
        self.meta.shown = !self.meta.shown;
    }

    /// The Metadata panel is shown.
    pub fn metadata_shown(&self) -> bool {
        self.meta.shown && self.mode == Mode::Library
    }

    /// The presets row: apply one to the selection, or save the first photo's as one.
    fn meta_presets_ui(
        &mut self,
        ui: &mut egui::Ui,
        ids: &[i64],
        first: lf_catalog::meta::Meta,
        env: &mut Env,
    ) {
        let mut apply = None;
        ui.horizontal(|ui| {
            let combo = egui::ComboBox::from_id_salt("meta-preset")
                .selected_text(APPLY_PRESET)
                .show_ui(ui, |ui| {
                    for (i, p) in env.prefs.meta_presets.iter().enumerate() {
                        if ui.selectable_label(false, &p.name).clicked() {
                            apply = Some(i);
                        }
                    }
                });
            combo
                .response
                .widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, PRESET));
            if ui
                .small_button(SAVE_PRESET)
                .on_hover_text(PRESET_HINT)
                .clicked()
            {
                self.meta.preset_name = Some(String::new());
            }
        });
        if let Some(name) = &mut self.meta.preset_name {
            let mut done = None;
            ui.horizontal(|ui| {
                let field = ui.add(egui::TextEdit::singleline(name).hint_text(PRESET_NAME));
                field.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::TextInput, true, PRESET_NAME)
                });
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui.button("Save").clicked() || enter) && !name.trim().is_empty() {
                    done = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    done = Some(false);
                }
            });
            if let Some(save) = done {
                if save {
                    let preset = crate::prefs::MetaPreset {
                        name: name.trim().to_owned(),
                        creator: first.creator.clone(),
                        copyright: first.copyright.clone(),
                        keywords: first.keywords.clone(),
                    };
                    env.prefs.meta_presets.retain(|p| p.name != preset.name);
                    env.prefs.meta_presets.push(preset);
                    env.save_prefs();
                }
                self.meta.preset_name = None;
            }
        }
        if let Some(p) = apply.and_then(|i| env.prefs.meta_presets.get(i).cloned()) {
            for change in preset_changes(&p) {
                self.change_meta(ids.to_vec(), change, env);
            }
            // The fields show the new values.
            self.meta.ids.clear();
        }
    }

    /// Changes the metadata of `ids` in memory and saves it.
    pub fn change_meta(&mut self, ids: Vec<i64>, change: Change, env: &mut Env) {
        if ids.is_empty() {
            return;
        }
        for id in &ids {
            if let Some(&i) = self.rows.get(id) {
                change.apply(&mut self.photos[i].meta);
            }
        }
        if self.filter.narrows() {
            self.arrange();
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        env.jobs.spawn(Lane::Io, "metadata", move || {
            let result = catalog_ext::change_meta(&root, &ids, &change);
            Msg::MetaSaved { root, result }
        });
    }

    /// The save of a change ended; a failure shows what is saved.
    pub(super) fn meta_saved(
        &mut self,
        root: std::path::PathBuf,
        result: Result<usize, String>,
        env: &mut Env,
    ) {
        if self.root.as_deref() != Some(root.as_path()) {
            return;
        }
        if let Err(e) = result {
            env.toasts.push(Level::Error, format!("{SAVE_FAILED} {e}"));
            self.reload_photos(env);
        }
    }

    /// The panel's contents.
    pub(super) fn metadata_panel(&mut self, ui: &mut egui::Ui, env: &mut Env) {
        ui.add_space(6.0);
        ui.heading(TITLE);
        ui.add_space(4.0);
        let ids = self.selected_in_order();
        if ids.is_empty() {
            ui.label(RichText::new(NOTHING_SELECTED).color(theme::MUTED));
            self.meta.ids.clear();
            return;
        }
        let metas: Vec<lf_catalog::meta::Meta> = ids
            .iter()
            .filter_map(|id| self.photo(*id).map(|r| r.meta.clone()))
            .collect();
        if ids.len() > 1 {
            ui.label(
                RichText::new(format!(
                    "{} photos selected",
                    crate::catalog_ext::count(ids.len())
                ))
                .color(theme::MUTED),
            );
        }
        self.meta_presets_ui(ui, &ids, metas.first().cloned().unwrap_or_default(), env);
        if ids.len() > 1
            && let Some(active) = self.selection.active()
            && ui.button(SYNC).on_hover_text(SYNC_HINT).clicked()
            && let Some(from) = self.photo(active).map(|p| p.meta.clone())
        {
            let others: Vec<i64> = ids.iter().copied().filter(|&i| i != active).collect();
            for change in sync_changes(&from) {
                self.change_meta(others.clone(), change, env);
            }
            self.meta.ids.clear();
        }
        // The value the selection agrees on (None: they differ).
        let common = |field: Field| -> Option<String> {
            let first = metas.first().and_then(|m| field.of(m)).unwrap_or_default();
            metas
                .iter()
                .all(|m| field.of(m).unwrap_or_default() == first)
                .then(|| first.to_owned())
        };
        if self.meta.ids != ids {
            self.meta.ids = ids.clone();
            self.meta.adding.clear();
            self.meta.drafts = Field::ALL
                .into_iter()
                .map(|f| {
                    let v = common(f).unwrap_or_default();
                    (f, v.clone(), v)
                })
                .collect();
        }
        let differs: Vec<bool> = Field::ALL.iter().map(|f| common(*f).is_none()).collect();
        // Keywords: on every selected photo, or on some (with how many).
        let mut keywords: Vec<(String, usize)> = Vec::new();
        for m in &metas {
            for k in &m.keywords {
                match keywords
                    .iter_mut()
                    .find(|(w, _)| w.to_lowercase() == k.to_lowercase())
                {
                    Some((_, n)) => *n += 1,
                    None => keywords.push((k.clone(), 1)),
                }
            }
        }
        keywords.sort_by_key(|(k, _)| k.to_lowercase());
        let total = metas.len();

        let mut changes: Vec<Change> = Vec::new();
        for (i, (field, draft, started)) in self.meta.drafts.iter_mut().enumerate() {
            ui.label(RichText::new(field.name()).color(theme::MUTED));
            let hint = if differs[i] { SEVERAL } else { "" };
            let edit = if *field == Field::Caption {
                egui::TextEdit::multiline(draft).desired_rows(3)
            } else {
                egui::TextEdit::singleline(draft)
            };
            let response = ui.add(edit.hint_text(hint).desired_width(f32::INFINITY));
            let name = field.name();
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, name));
            if response.lost_focus() && draft != started {
                changes.push(Change::Set(*field, draft.clone()));
                *started = draft.clone();
            }
            ui.add_space(2.0);
        }

        ui.add_space(6.0);
        ui.label(RichText::new("Keywords").color(theme::MUTED));
        let mut remove = None;
        ui.horizontal_wrapped(|ui| {
            for (word, n) in &keywords {
                let text = if *n == total {
                    format!("{word} ×")
                } else {
                    format!("{word} ({n}/{total}) ×")
                };
                if ui
                    .small_button(text)
                    .on_hover_text("Remove from the selected photos")
                    .clicked()
                {
                    remove = Some(word.clone());
                }
            }
        });
        if let Some(word) = remove {
            changes.push(Change::RemoveKeywords(vec![word]));
        }
        let add = ui.add(
            egui::TextEdit::singleline(&mut self.meta.adding)
                .hint_text(ADD_HINT)
                .desired_width(f32::INFINITY),
        );
        add.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, ADD_KEYWORDS));
        if add.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            let words = parse_keywords(&self.meta.adding);
            if !words.is_empty() {
                changes.push(Change::AddKeywords(words));
            }
            self.meta.adding.clear();
            add.request_focus();
        }

        // AI keywords of the selection not yet among its keywords: a click keeps one.
        let mut suggested: Vec<String> = Vec::new();
        for id in &ids {
            if let Some(row) = self.photo(*id) {
                for w in &row.ai_keywords {
                    let taken = keywords.iter().any(|(k, _)| k.eq_ignore_ascii_case(w));
                    if !taken && !suggested.iter().any(|s| s.eq_ignore_ascii_case(w)) {
                        suggested.push(w.clone());
                    }
                }
            }
        }
        if !suggested.is_empty() {
            ui.add_space(6.0);
            ui.label(RichText::new(SUGGESTED).color(theme::MUTED));
            ui.horizontal_wrapped(|ui| {
                for word in &suggested {
                    if ui
                        .small_button(format!("+ {word}"))
                        .on_hover_text("Keep as a keyword of the selected photos")
                        .clicked()
                    {
                        changes.push(Change::AddKeywords(vec![word.clone()]));
                    }
                }
            });
        }

        // The folder's keywords: a click puts one on the selection.
        let mut in_folder: Vec<(String, usize)> = Vec::new();
        for row in &self.photos {
            for k in &row.meta.keywords {
                match in_folder
                    .iter_mut()
                    .find(|(w, _)| w.to_lowercase() == k.to_lowercase())
                {
                    Some((_, n)) => *n += 1,
                    None => in_folder.push((k.clone(), 1)),
                }
            }
        }
        if !in_folder.is_empty() {
            in_folder.sort_by_key(|(k, _)| k.to_lowercase());
            ui.add_space(8.0);
            ui.label(RichText::new(IN_FOLDER).color(theme::MUTED));
            egui::ScrollArea::vertical()
                .id_salt("folder-keywords")
                .max_height(160.0)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for (word, n) in &in_folder {
                            if ui
                                .small_button(format!("{word} {n}"))
                                .on_hover_text("Add to the selected photos")
                                .clicked()
                            {
                                changes.push(Change::AddKeywords(vec![word.clone()]));
                            }
                        }
                    });
                });
        }
        for change in changes {
            self.change_meta(ids.clone(), change, env);
        }
    }
}
