//! Develop's Looks, Studio presets and Versions sections, above the adjust panels, and their
//! dialogs. Each change to the settings goes through [`Develop::replace_settings`] (the one
//! "settings changed" entry point, then one commit); versions go through the edit session's
//! catalogue connection and hand their sidecar to the writer.
//!
//! Version previews are rendered with `Renderer::pixels(settings, 112)` on the UI thread,
//! only for the versions on screen while the section is open, once each (cached per version
//! id until the photo changes).

use super::panels::{looks, presets, versions};
use super::{Develop, State};
use crate::dialogs::{self, save_preset};
use crate::toast::{Level, Toasts};
use eframe::egui;
use lf_catalog::presets::{self as local, LocalPresetRow};
use lf_catalog::{Version, edits, versions as catalog_versions};
use lf_settings::EditSettings;
use lf_settings::groups::{apply_groups, groups_in, pick_groups};
use lf_settings::looks::{LOOKS, apply_look};
use std::collections::HashMap;

/// The presets section's state.
#[derive(Default)]
pub struct PresetsUi {
    /// The studio whose presets are shown (they never mix between studios).
    studio: Option<i64>,
    rows: Vec<LocalPresetRow>,
    /// Read the rows again before showing them.
    stale: bool,
    saving: Option<save_preset::SavePresetDialog>,
    deleting: Option<LocalPresetRow>,
    /// Saved or deleted here since the last sync was asked for.
    changed: bool,
}

/// The versions section's state.
#[derive(Default)]
pub struct VersionsUi {
    /// The photo the list and previews belong to.
    photo: Option<i64>,
    list: Vec<Version>,
    stale: bool,
    /// Previews by version id (`None`: it could not be made; not tried again).
    thumbs: HashMap<String, Option<egui::TextureHandle>>,
    adding: Option<String>,
    using: Option<String>,
    deleting: Option<String>,
}

#[derive(Default)]
pub struct Extras {
    pub presets: PresetsUi,
    pub versions: VersionsUi,
}

impl Develop {
    /// The studio signed in to (its presets are shown); `None` when there is none.
    pub fn set_studio(&mut self, studio: Option<i64>) {
        let p = &mut self.extras.presets;
        if p.studio != studio {
            p.studio = studio;
            p.rows.clear();
            p.stale = true;
        }
    }

    /// The catalogue the presets, versions and pastes use (also before any photo is open).
    pub fn set_root(&mut self, root: &std::path::Path) {
        self.session.set_root(root);
        self.extras.presets.stale = true;
    }

    /// A preset was saved or deleted here since the last call: the library asks for a sync.
    pub fn take_presets_changed(&mut self) -> bool {
        std::mem::take(&mut self.extras.presets.changed)
    }

    /// The presets changed in the catalogue (a sync): read them again.
    pub fn presets_stale(&mut self) {
        self.extras.presets.stale = true;
    }

    /// The studio's presets, read again when stale.
    pub fn preset_rows(&mut self) -> Vec<LocalPresetRow> {
        let p = &mut self.extras.presets;
        if let (true, Some(studio)) = (p.stale, p.studio) {
            match self
                .session
                .catalog()
                .and_then(|c| local::list(c, studio).map_err(|e| e.to_string()))
            {
                Ok(rows) => p.rows = rows,
                Err(e) => log::warn!("could not read the studio presets: {e}"),
            }
            p.stale = false;
        }
        p.rows.clone()
    }

    /// Replaces the open photo's settings with `next` and records it (one history step).
    pub fn replace_settings(&mut self, next: EditSettings, toasts: &mut Toasts) {
        self.edit(|s| *s = next);
        self.commit(toasts);
    }

    /// A photo's settings from the catalogue (the open photo's live ones when it is open).
    pub fn photo_settings(&mut self, photo_id: i64) -> Option<EditSettings> {
        if let Some(edit) = self.session.edit().filter(|e| e.photo_id == photo_id) {
            return Some(edit.settings.clone());
        }
        let catalog = self.session.catalog().ok()?;
        match edits::get_settings(catalog, photo_id) {
            Ok(s) => Some(s.unwrap_or_else(EditSettings::default_settings)),
            Err(e) => {
                log::warn!("could not read the settings of photo {photo_id}: {e}");
                None
            }
        }
    }

    /// Pastes `partial` onto one photo that is not open, at once (its sidecar is written
    /// now). True when it was recorded.
    pub fn apply_to_photo(
        &mut self,
        photo_id: i64,
        partial: &serde_json::Value,
        toasts: &mut Toasts,
    ) -> bool {
        let saver = self.saver();
        let result = self
            .session
            .catalog()
            .and_then(|c| crate::dialogs::batch::apply_one(c, photo_id, partial, &saver));
        match result {
            Ok(()) => true,
            Err(e) => {
                toasts.push(Level::Error, format!("The edits could not be pasted. {e}"));
                false
            }
        }
    }

    /// The open photo's settings changed in the catalogue behind it (a batch): take them.
    pub fn reload_edit(&mut self, toasts: &mut Toasts) {
        let damaged = self.sidecar_damaged();
        let path = self.photo_path.clone();
        let reloaded = self.session.with_catalog(
            "The photo's edit could not be read again.",
            path.as_deref(),
            damaged,
            toasts,
            |c, edit| {
                let s = edits::get_settings(c, edit.photo_id)?
                    .unwrap_or_else(EditSettings::default_settings);
                edit.adopt(s, c);
                Ok(None)
            },
        );
        if reloaded {
            self.settings_changed();
            self.extras.versions.stale = true;
        }
    }

    fn sidecar_damaged(&self) -> bool {
        matches!(&self.state, State::Ready(o, _) if o.sidecar_error.is_some())
    }

    /// The Looks, Studio presets and Versions sections (closed at first).
    pub(super) fn extras_ui(&mut self, ui: &mut egui::Ui, toasts: &mut Toasts) {
        if !self.is_ready() {
            return;
        }
        egui::CollapsingHeader::new(looks::TITLE)
            .id_salt("looks-section")
            .show(ui, |ui| {
                if let Some(i) = looks::show(ui)
                    && let Some(edit) = self.session.edit()
                {
                    let next = apply_look(&edit.settings, &LOOKS[i].settings);
                    self.replace_settings(next, toasts);
                }
            });
        egui::CollapsingHeader::new(presets::TITLE)
            .id_salt("presets-section")
            .show(ui, |ui| self.presets_ui(ui, toasts));
        egui::CollapsingHeader::new(versions::TITLE)
            .id_salt("versions-section")
            .show(ui, |ui| self.versions_ui(ui, toasts));
        self.extras_dialogs(ui.ctx(), toasts);
        ui.separator();
    }

    fn presets_ui(&mut self, ui: &mut egui::Ui, toasts: &mut Toasts) {
        if self.extras.presets.studio.is_none() {
            ui.label(egui::RichText::new(presets::NO_STUDIO).small());
            return;
        }
        let rows = self.preset_rows();
        match presets::show(ui, &rows) {
            Some(presets::PresetAction::Apply(i)) => {
                if let (Some(row), Some(edit)) = (rows.get(i), self.session.edit()) {
                    let groups = groups_in(&row.settings);
                    let partial = crate::clipboard::clipboard_settings(&row.settings);
                    let next = apply_groups(&edit.settings, &partial, &groups);
                    self.replace_settings(next, toasts);
                }
            }
            Some(presets::PresetAction::Save) => {
                self.extras.presets.saving = Some(save_preset::SavePresetDialog::default());
            }
            Some(presets::PresetAction::Delete(i)) => {
                self.extras.presets.deleting = rows.get(i).cloned();
            }
            None => {}
        }
    }

    fn versions_ui(&mut self, ui: &mut egui::Ui, toasts: &mut Toasts) {
        self.load_versions();
        let current = self.session.edit().map(|e| e.settings.clone());
        let v = &self.extras.versions;
        let rows: Vec<versions::Row> = v
            .list
            .iter()
            .map(|version| versions::Row {
                version,
                thumb: v
                    .thumbs
                    .get(&version.id)
                    .and_then(Option::as_ref)
                    .map(|t| (t.id(), t.size_vec2())),
                current: current.as_ref() == Some(&version.settings),
            })
            .collect();
        let mut wanted = Vec::new();
        let action = versions::show(ui, &rows, &mut wanted);
        // Only versions not tried yet (a failed preview is not tried again).
        wanted.retain(|id| !v.thumbs.contains_key(id));
        drop(rows);
        match action {
            Some(versions::VersionAction::Add) => {
                self.extras.versions.adding =
                    Some(versions::default_label(self.extras.versions.list.len()));
            }
            Some(versions::VersionAction::Use(id)) => self.extras.versions.using = Some(id),
            Some(versions::VersionAction::Delete(id)) => self.extras.versions.deleting = Some(id),
            None => {}
        }
        self.make_thumbs(ui.ctx(), &wanted, toasts);
    }

    /// Reads the open photo's versions when the photo changed or they did.
    fn load_versions(&mut self) {
        let photo = self.session.edit().map(|e| e.photo_id);
        let v = &mut self.extras.versions;
        if v.photo != photo {
            *v = VersionsUi {
                photo,
                stale: true,
                ..VersionsUi::default()
            };
        }
        let Some(photo) = photo.filter(|_| v.stale) else {
            return;
        };
        v.stale = false;
        match self
            .session
            .catalog()
            .and_then(|c| catalog_versions::list(c, photo).map_err(|e| e.to_string()))
        {
            Ok(list) => self.extras.versions.list = list,
            Err(e) => log::warn!("could not read the versions of photo {photo}: {e}"),
        }
    }

    /// Renders the previews of `ids` (the versions on screen without one), once each.
    fn make_thumbs(&mut self, ctx: &egui::Context, ids: &[String], toasts: &mut Toasts) {
        for id in ids {
            let Some(settings) = self
                .extras
                .versions
                .list
                .iter()
                .find(|v| &v.id == id)
                .map(|v| v.settings.clone())
            else {
                continue;
            };
            let State::Ready(opened, _) = &mut self.state else {
                return;
            };
            let made = match opened.renderer.pixels(&settings, versions::THUMB) {
                Ok(image) => {
                    let size = [image.width() as usize, image.height() as usize];
                    let pixels = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
                    Some(ctx.load_texture(
                        format!("version-{id}"),
                        pixels,
                        egui::TextureOptions::LINEAR,
                    ))
                }
                Err(e) => {
                    self.renderer_error("A version's preview could not be made", e, toasts);
                    None
                }
            };
            self.extras.versions.thumbs.insert(id.clone(), made);
            if self.is_lost() {
                return;
            }
        }
    }

    fn extras_dialogs(&mut self, ctx: &egui::Context, toasts: &mut Toasts) {
        self.save_preset_dialog(ctx, toasts);
        if let Some(row) = self.extras.presets.deleting.clone() {
            let answer = dialogs::confirm(
                ctx,
                "delete-preset",
                &presets::delete_title(&row.name),
                presets::DELETE_TEXT,
                presets::DELETE_CONFIRM,
                true,
            );
            if let Some(yes) = answer {
                self.extras.presets.deleting = None;
                if yes {
                    match self
                        .session
                        .catalog()
                        .and_then(|c| local::delete(c, row.id).map_err(|e| e.to_string()))
                    {
                        Ok(()) => {
                            self.extras.presets.stale = true;
                            self.extras.presets.changed = true;
                        }
                        Err(e) => toasts.push(
                            Level::Error,
                            format!("The preset could not be deleted. {e}"),
                        ),
                    }
                }
            }
        }
        self.add_version_dialog(ctx, toasts);
        if let Some(id) = self.extras.versions.using.clone() {
            let answer = dialogs::confirm(
                ctx,
                "use-version",
                versions::USE_TITLE,
                versions::USE_QUESTION,
                versions::USE_CONFIRM,
                false,
            );
            if let Some(yes) = answer {
                self.extras.versions.using = None;
                if yes {
                    self.use_version(&id, toasts);
                }
            }
        }
        if let Some(id) = self.extras.versions.deleting.clone() {
            let label = self
                .extras
                .versions
                .list
                .iter()
                .find(|v| v.id == id)
                .map(|v| v.label.clone())
                .unwrap_or_default();
            let answer = dialogs::confirm(
                ctx,
                "delete-version",
                &versions::delete_title(&label),
                versions::DELETE_TEXT,
                versions::DELETE_CONFIRM,
                true,
            );
            if let Some(yes) = answer {
                self.extras.versions.deleting = None;
                if yes {
                    self.delete_version(&id, toasts);
                }
            }
        }
    }

    fn save_preset_dialog(&mut self, ctx: &egui::Context, toasts: &mut Toasts) {
        let (Some(dialog), Some(studio)) = (
            self.extras.presets.saving.as_mut(),
            self.extras.presets.studio,
        ) else {
            return;
        };
        match dialog.show(ctx) {
            save_preset::SaveAction::None => {}
            save_preset::SaveAction::Cancel => self.extras.presets.saving = None,
            save_preset::SaveAction::Save { name, groups } => {
                let Some(settings) = self.session.edit().map(|e| e.settings.clone()) else {
                    return;
                };
                let partial = pick_groups(&settings, &groups);
                let result = self.session.catalog().and_then(|c| {
                    match local::find(c, studio, &name).map_err(|e| e.to_string())? {
                        Some(row) if row.deleted_at.is_none() => Ok(false),
                        _ => local::save(c, studio, &name, &partial)
                            .map(|_| true)
                            .map_err(|e| e.to_string()),
                    }
                });
                match result {
                    Ok(true) => {
                        self.extras.presets.saving = None;
                        self.extras.presets.stale = true;
                        self.extras.presets.changed = true;
                        toasts.push(Level::Success, format!("Preset “{name}” saved."));
                    }
                    Ok(false) => {
                        if let Some(d) = self.extras.presets.saving.as_mut() {
                            d.error = Some(save_preset::TAKEN.to_owned());
                        }
                    }
                    Err(e) => {
                        toasts.push(Level::Error, format!("The preset could not be saved. {e}"))
                    }
                }
            }
        }
    }

    fn add_version_dialog(&mut self, ctx: &egui::Context, toasts: &mut Toasts) {
        let Some(label) = self.extras.versions.adding.as_mut() else {
            return;
        };
        let mut answer = None;
        let _ = egui::Modal::new(egui::Id::unique("add-version")).show(ctx, |ui| {
            ui.set_max_width(380.0);
            ui.heading(versions::ADD_TITLE);
            ui.add_space(8.0);
            let name = ui.label(versions::LABEL);
            let field = ui
                .add(egui::TextEdit::singleline(label).char_limit(versions::MAX_LABEL))
                .labelled_by(name.id);
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.add_space(12.0);
            let can = !label.trim().is_empty();
            ui.horizontal(|ui| {
                if (ui
                    .add_enabled(can, egui::Button::new(versions::SAVE))
                    .clicked()
                    || (enter && can))
                    && answer.is_none()
                {
                    answer = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    answer = Some(false);
                }
            });
        });
        if answer.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            answer = Some(false);
        }
        match answer {
            Some(true) => {
                let label: String = label.trim().chars().take(versions::MAX_LABEL).collect();
                self.extras.versions.adding = None;
                self.add_version(&label, toasts);
            }
            Some(false) => self.extras.versions.adding = None,
            None => {}
        }
    }

    /// Keeps the current settings as a version (a gesture in progress is committed first).
    pub fn add_version(&mut self, label: &str, toasts: &mut Toasts) {
        let damaged = self.sidecar_damaged();
        let path = self.photo_path.clone();
        let label = label.to_owned();
        if self.session.with_catalog(
            "The version could not be added.",
            path.as_deref(),
            damaged,
            toasts,
            |c, edit| {
                edit.commit(c)?;
                catalog_versions::add(c, edit.photo_id, &label, &edit.settings)?;
                edits::sidecar_for(c, edit.photo_id).map(Some)
            },
        ) {
            self.extras.versions.stale = true;
        }
    }

    /// Makes a version's settings current (recorded in history, so undo goes back).
    pub fn use_version(&mut self, id: &str, toasts: &mut Toasts) {
        let damaged = self.sidecar_damaged();
        let path = self.photo_path.clone();
        if self.session.with_catalog(
            "The version could not be used.",
            path.as_deref(),
            damaged,
            toasts,
            |c, edit| {
                edit.commit(c)?;
                let sidecar = catalog_versions::use_version(c, edit.photo_id, id)?;
                edit.adopt(sidecar.settings.clone(), c);
                Ok(Some(sidecar))
            },
        ) {
            self.settings_changed();
            self.panels.curve.clear_selection();
            self.tools.clear_selection();
            self.check_develop_change();
        }
    }

    pub fn delete_version(&mut self, id: &str, toasts: &mut Toasts) {
        let damaged = self.sidecar_damaged();
        let path = self.photo_path.clone();
        if self.session.with_catalog(
            "The version could not be deleted.",
            path.as_deref(),
            damaged,
            toasts,
            |c, edit| {
                catalog_versions::delete(c, edit.photo_id, id)?;
                edits::sidecar_for(c, edit.photo_id).map(Some)
            },
        ) {
            self.extras.versions.stale = true;
            self.extras.versions.thumbs.remove(id);
        }
    }

    /// How many version previews have been made for the open photo (each at most once).
    pub fn version_previews(&self) -> usize {
        self.extras.versions.thumbs.len()
    }

    /// The open photo's versions (as last read).
    pub fn versions(&mut self) -> &[Version] {
        self.load_versions();
        &self.extras.versions.list
    }
}
