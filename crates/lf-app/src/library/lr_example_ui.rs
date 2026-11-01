//! "Match a Lightroom example…" on the library's side: the dialog, its pickers (each on its own
//! short-lived thread, as the other pickers), the match on the `Long` lane
//! ([`crate::lightroom::example_job::run`], on the shared device), and saving the result as a
//! studio preset (over the preset matched, or as a new one), which asks for a sync as a saved
//! preset does. The table the preset carries is written to the catalogue's LUT folder on save.

use super::{Env, Library, Mode};
use crate::dialogs::lr_example::{self, LrExampleAction, LrExampleDialog, Matched, Pick, Stage};
use crate::jobs::{Lane, Msg};
use crate::lightroom::import::unique_name;
use crate::toast::Level;
use eframe::egui;
use lf_settings::lightroom::budget::desktop_fits;
use std::path::PathBuf;

/// The job's name (a panic in it is reported under this name).
pub const JOB: &str = "match a Lightroom example";

/// "Preset “Moody (matched)” saved."
pub fn saved_text(name: &str) -> String {
    format!("Preset “{name}” saved.")
}

impl Library {
    /// Whether "Match a Lightroom example…" can open: a studio to save into and a catalogue
    /// (for the table's file), as the import.
    pub(super) fn can_match_lr(&self) -> bool {
        self.can_import_lr()
    }

    /// The dialog, while open (tests answer its pickers by its id).
    pub fn lr_example(&self) -> Option<&LrExampleDialog> {
        self.lr_example.as_ref()
    }

    /// Opens the dialog for `preset` (its name and settings) or a new preset, with the photo
    /// open in Develop (else the active one) as the original.
    pub(super) fn open_lr_example(&mut self, preset: Option<(String, serde_json::Value)>) {
        if self.lr_example.is_some() {
            return;
        }
        let photo = match self.mode {
            Mode::Develop(id) => Some(id),
            Mode::Library => self.selection.active(),
        };
        let original = photo.and_then(|id| self.photo(id)).map(|p| p.path.clone());
        self.lr_example = Some(LrExampleDialog::new(preset, original));
    }

    pub(super) fn lr_example_dialog(&mut self, ctx: &egui::Context, env: &mut Env) {
        let Some(dialog) = &mut self.lr_example else {
            return;
        };
        match dialog.show(ctx) {
            LrExampleAction::None => {}
            LrExampleAction::Close => {
                dialog.stop.store(true, std::sync::atomic::Ordering::SeqCst);
                self.lr_example = None;
            }
            LrExampleAction::Choose(which) => pick(dialog.id, which, env),
            LrExampleAction::Match => self.start_lr_example(env),
            LrExampleAction::Save => self.save_lr_example(false, env),
            LrExampleAction::SaveAsNew => self.save_lr_example(true, env),
        }
    }

    fn start_lr_example(&mut self, env: &mut Env) {
        let Some(dialog) = self.lr_example.as_mut() else {
            return;
        };
        let (Some(original), Some(export)) = (dialog.original.clone(), dialog.export.clone())
        else {
            return;
        };
        dialog.stage = Stage::Matching;
        dialog.error = None;
        let (id, stop, preset) = (
            dialog.id,
            dialog.stop.clone(),
            dialog.preset.as_ref().map(|(_, p)| p.clone()),
        );
        let name = dialog.base_name();
        let gpu = self.gpu.clone();
        env.jobs.spawn(Lane::Long, JOB, move || {
            let result = crate::lightroom::example_job::run(
                &gpu,
                &original,
                &export,
                preset.as_ref(),
                &name,
                &stop,
            )
            .map(|(preset, fit, previews)| {
                Box::new(Matched {
                    preset,
                    fit,
                    previews,
                })
            });
            Msg::LrExampleFitted { dialog: id, result }
        });
    }

    /// A picker answered for the dialog `id` (an earlier dialog's answer is dropped).
    pub(super) fn lr_example_picked(&mut self, id: u64, which: Pick, path: Option<PathBuf>) {
        if let (Some(dialog), Some(path)) = (&mut self.lr_example, path)
            && dialog.id == id
            && dialog.stage == Stage::Choosing
        {
            match which {
                Pick::Original => dialog.original = Some(path),
                Pick::Export => dialog.export = Some(path),
            }
        }
    }

    /// The match for the dialog `id` ended (an answer for a closed dialog is dropped).
    pub(super) fn lr_example_fitted(&mut self, id: u64, result: Result<Box<Matched>, String>) {
        if let Some(dialog) = &mut self.lr_example
            && dialog.id == id
            && dialog.stage == Stage::Matching
        {
            dialog.finished(result);
        }
    }

    /// The match job panicked: the dialog stops waiting and says so.
    pub(super) fn lr_example_failed(&mut self, plain: &str) {
        if let Some(dialog) = &mut self.lr_example
            && dialog.stage == Stage::Matching
        {
            dialog.finished(Err(plain.to_owned()));
        }
    }

    /// Saves the match over the preset matched, or `as_new` under "<Name> (matched)" (numbered
    /// when taken), writes its table's file, and closes the dialog; a failure stays in it.
    fn save_lr_example(&mut self, as_new: bool, env: &mut Env) {
        let (Some(studio), Some(root)) = (self.develop.studio(), self.root.clone()) else {
            return;
        };
        let Some(dialog) = self.lr_example.as_ref() else {
            return;
        };
        let Stage::Done(matched) = &dialog.stage else {
            return;
        };
        let preset = matched.preset.clone();
        let replacing = dialog.preset.as_ref().map(|(name, _)| name.clone());
        let base = dialog.base_name();
        if !desktop_fits(&preset) {
            if let Some(d) = self.lr_example.as_mut() {
                d.error = Some(lr_example::TOO_LARGE.to_owned());
            }
            return;
        }
        let saved = match replacing.filter(|_| !as_new) {
            Some(name) => self
                .develop
                .replace_preset(studio, &name, &preset)
                .map(|()| name),
            None => {
                let taken = self
                    .develop
                    .preset_rows()
                    .into_iter()
                    .map(|r| r.name)
                    .collect();
                let name = unique_name(&lr_example::new_name(&base), &taken);
                match self.develop.save_preset(studio, &name, &preset) {
                    Ok(true) => Ok(name),
                    Ok(false) => Err(crate::dialogs::save_preset::TAKEN.to_owned()),
                    Err(e) => Err(e),
                }
            }
        };
        match saved {
            Ok(name) => {
                if let Err(e) = crate::luts::write_embedded(&root, &preset) {
                    // The preset carries its table: applying it writes the file again.
                    log::warn!("the matched preset's table was not written: {e}");
                }
                self.lr_example = None;
                env.toasts.push(Level::Success, saved_text(&name));
            }
            Err(e) => {
                if let Some(d) = self.lr_example.as_mut() {
                    d.error = Some(format!("{} {e}", crate::lightroom::import::NOT_SAVED));
                }
            }
        }
    }
}

/// Opens the picker for the original photo or Lightroom's export on its own thread for the
/// dialog `id`.
fn pick(id: u64, which: Pick, env: &mut Env) {
    let reporter = env.jobs.reporter();
    let spawned = std::thread::Builder::new()
        .name("lf-picker".into())
        .spawn(move || {
            let dialog = rfd::FileDialog::new().set_title(lr_example::TITLE);
            let path = match which {
                Pick::Original => dialog.pick_file(),
                Pick::Export => dialog
                    .add_filter(lr_example::EXPORT, lr_example::EXPORT_FILES)
                    .pick_file(),
            };
            log::info!("Lightroom example picker answered: {path:?}");
            reporter.send(Msg::LrExamplePicked {
                dialog: id,
                pick: which,
                path,
            });
        });
    if let Err(e) = spawned {
        log::warn!("could not open the file picker: {e}");
    }
}
