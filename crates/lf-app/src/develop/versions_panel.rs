//! Develop's Versions section: named snapshots of the open photo's settings (add, use,
//! delete) through the edit session's catalogue connection; each change hands its sidecar to
//! the writer.
//!
//! Version previews are rendered with `Renderer::pixels(settings, 112)` on the UI thread,
//! only for the versions on screen while the section is open, once each (cached per version
//! id until the photo changes).

use super::panels::versions;
use super::{Develop, State};
use crate::dialogs;
use crate::toast::Toasts;
use eframe::egui;
use lf_catalog::{Version, edits, versions as catalog_versions};
use std::collections::HashMap;

/// The versions section's state.
#[derive(Default)]
pub struct VersionsUi {
    /// The photo the list and previews belong to.
    pub(super) photo: Option<i64>,
    pub(super) list: Vec<Version>,
    pub(super) stale: bool,
    /// Previews by version id (`None`: it could not be made; not tried again).
    pub(super) thumbs: HashMap<String, Option<egui::TextureHandle>>,
    pub(super) adding: Option<String>,
    pub(super) using: Option<String>,
    pub(super) deleting: Option<String>,
}

impl Develop {
    pub(super) fn versions_ui(&mut self, ui: &mut egui::Ui, toasts: &mut Toasts) {
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

    pub(super) fn version_dialogs(&mut self, ctx: &egui::Context, toasts: &mut Toasts) {
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

    /// Photo → Add version…: asks for the version's name, as the section's button does.
    pub fn ask_add_version(&mut self) {
        if self.session.edit().is_none() {
            return;
        }
        self.load_versions();
        self.extras.versions.adding =
            Some(versions::default_label(self.extras.versions.list.len()));
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
