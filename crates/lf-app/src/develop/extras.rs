//! Develop's Looks and Studio presets sections, above the adjust panels, and their dialogs
//! (the Versions section is in `versions_panel.rs`). Each change to the settings goes through
//! [`Develop::replace_settings`] (the one "settings changed" entry point, then one commit).
//! The look or preset applied last keeps an Amount slider (0–200 %, `lf_settings::amount`)
//! until the photo is edited some other way; dragging it is one gesture (one history step).

use super::panels::{looks, presets, versions};
use super::{Develop, State};
use crate::dialogs::{self, save_preset};
use crate::toast::{Level, Toasts};
use eframe::egui;
use lf_catalog::edits;
use lf_catalog::presets::{self as local, LocalPresetRow};
use lf_settings::EditSettings;
use lf_settings::groups::pick_groups;
use lf_settings::looks::{LOOKS, apply_look};

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

/// The look or preset applied last, while its Amount can still change.
#[derive(Debug, Clone)]
pub struct Applied {
    name: String,
    photo_id: i64,
    /// The photo's settings before it, and with it at 100 %.
    before: EditSettings,
    after: EditSettings,
    amount: f32,
    /// What the slider last made: any other change to the photo ends the slider.
    last: EditSettings,
}

pub const AMOUNT: &str = "Amount";

#[derive(Default)]
pub struct Extras {
    pub presets: PresetsUi,
    pub versions: super::versions_panel::VersionsUi,
    pub applied: Option<Applied>,
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

    /// Saves `partial` as the studio's preset `name` (and asks for a sync): `Ok(false)` when
    /// a live preset already has that name. `studio` is the one signed in to.
    pub fn save_preset(
        &mut self,
        studio: i64,
        name: &str,
        partial: &serde_json::Value,
    ) -> Result<bool, String> {
        let saved = self.session.patient(|c| {
            match local::find(c, studio, name).map_err(|e| e.to_string())? {
                Some(row) if row.deleted_at.is_none() => Ok(false),
                _ => local::save(c, studio, name, partial)
                    .map(|_| true)
                    .map_err(|e| e.to_string()),
            }
        })?;
        if saved {
            self.extras.presets.stale = true;
            self.extras.presets.changed = true;
        }
        Ok(saved)
    }

    /// The studio whose presets are shown (signed in to), if any.
    pub fn studio(&self) -> Option<i64> {
        self.extras.presets.studio
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

    /// Pastes `partial` onto one photo that is not open, at once (its sidecar goes to the
    /// debounced writer, off the UI thread). True when it was recorded.
    pub fn apply_to_photo(
        &mut self,
        photo_id: i64,
        recipe: &crate::clipboard::Recipe,
        toasts: &mut Toasts,
    ) -> bool {
        let result = self.session.patient(|c| {
            crate::dialogs::batch::record(c, photo_id, |_, current| Ok(recipe.apply(current)))
        });
        match result {
            Ok(write) => {
                if let Some(write) = write {
                    self.session.save_later(write);
                }
                true
            }
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

    pub(super) fn sidecar_damaged(&self) -> bool {
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
                    self.apply_with_amount(LOOKS[i].name.to_owned(), next, toasts);
                }
            });
        egui::CollapsingHeader::new(presets::TITLE)
            .id_salt("presets-section")
            .show(ui, |ui| self.presets_ui(ui, toasts));
        self.amount_ui(ui, toasts);
        egui::CollapsingHeader::new(versions::TITLE)
            .id_salt("versions-section")
            .show(ui, |ui| self.versions_ui(ui, toasts));
        self.extras_dialogs(ui.ctx(), toasts);
        ui.separator();
    }

    /// Applies a look or preset (`next`, at 100 %) and offers its Amount.
    fn apply_with_amount(&mut self, name: String, next: EditSettings, toasts: &mut Toasts) {
        let Some(edit) = self.session.edit() else {
            return;
        };
        let (photo_id, before) = (edit.photo_id, edit.settings.clone());
        self.replace_settings(next.clone(), toasts);
        self.extras.applied = Some(Applied {
            name,
            photo_id,
            before,
            after: next.clone(),
            amount: 1.0,
            last: next,
        });
    }

    /// The Amount slider of the look or preset applied last, while nothing else changed the
    /// photo since.
    fn amount_ui(&mut self, ui: &mut egui::Ui, toasts: &mut Toasts) {
        let current = self
            .session
            .edit()
            .map(|e| (e.photo_id, e.settings.clone()));
        let Some(applied) = self.extras.applied.as_mut() else {
            return;
        };
        if current
            .as_ref()
            .is_none_or(|(id, s)| *id != applied.photo_id || *s != applied.last)
        {
            self.extras.applied = None;
            return;
        }
        let mut pct = applied.amount * 100.0;
        let response = ui
            .horizontal(|ui| {
                let label = ui.label(AMOUNT).on_hover_text(&applied.name);
                ui.add(
                    egui::Slider::new(&mut pct, 0.0..=lf_settings::amount::MAX_AMOUNT * 100.0)
                        .suffix(" %")
                        .step_by(1.0),
                )
                .labelled_by(label.id)
                .on_hover_text(format!("How much of {} to apply", applied.name))
            })
            .inner;
        let mut commit = response.drag_stopped();
        if response.changed() {
            applied.amount = pct / 100.0;
            let next = lf_settings::amount::blend(&applied.before, &applied.after, applied.amount);
            applied.last = next.clone();
            self.edit(|s| *s = next);
            // A typed or keyed change is a whole gesture at once.
            commit |= !response.dragged();
        }
        if commit {
            self.commit(toasts);
            // The committed settings are what the slider reads from now on.
            if let (Some(a), Some(e)) = (self.extras.applied.as_mut(), self.session.edit()) {
                a.last = e.settings.clone();
            }
        }
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
                    // As the web: a preset is applied as a look (its geometry and spots never).
                    let next =
                        crate::clipboard::Recipe::Look(row.settings.clone()).apply(&edit.settings);
                    self.apply_with_amount(row.name.clone(), next, toasts);
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
                        .patient(|c| local::delete(c, row.id).map_err(|e| e.to_string()))
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
        self.version_dialogs(ctx, toasts);
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
                match self.save_preset(studio, &name, &partial) {
                    Ok(true) => {
                        self.extras.presets.saving = None;
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
}
