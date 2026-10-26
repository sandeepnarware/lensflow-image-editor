//! The open photo's edit session: its [`EditState`], the catalogue connection that records
//! commits, undo and redo, the debounced sidecar writer, and the damaged-sidecar warning.
//!
//! The catalogue connection lives on the UI thread: each change is a single-row transaction
//! of a few milliseconds, undo and redo must change the settings the next frame draws (a
//! lane would answer later, and the `Io` lane can be busy with an import for minutes), and
//! a commit must be in the catalogue before a RAW reopen reads it on the `Open` lane.

use super::debounce::{PendingWrite, SidecarSaver};
use super::edit_state::EditState;
use crate::jobs::Jobs;
use crate::toast::{Level, Toasts};
use lf_catalog::{Catalog, Sidecar};
use lf_settings::groups::GroupKey;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The warning for a photo whose edit file cannot be read: `name` is the photo's file name.
pub fn damaged_text(name: &str) -> String {
    format!(
        "This photo's edit file is damaged, so edits are kept in the catalogue only. Fix or \
         remove {name}.lensflow.json to save beside the photo again."
    )
}

/// The toast when the sidecar beside a photo could not be written.
pub fn write_failed_text(message: &str) -> String {
    format!(
        "The edit file beside the photo could not be saved, so the edit is kept in the \
         catalogue only. {message}"
    )
}

/// The damaged-sidecar warning showing now.
#[derive(Debug, Clone)]
pub struct Damaged {
    pub photo_id: i64,
    /// The photo's file name.
    pub name: String,
    /// The edit file beside it.
    pub sidecar: PathBuf,
}

/// A change to the edits that goes through the catalogue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditOp {
    Commit,
    Undo,
    Redo,
    Reset(&'static [GroupKey]),
}

/// Opens the catalogue connection once.
fn connect<'a>(
    catalog: &'a mut Option<Catalog>,
    root: Option<&Path>,
) -> Result<&'a Catalog, String> {
    if catalog.is_none() {
        let root = root.ok_or_else(|| "no catalogue is open".to_owned())?;
        *catalog = Some(Catalog::open(root).map_err(|e| e.to_string())?);
    }
    catalog
        .as_ref()
        .ok_or_else(|| "no catalogue is open".to_owned())
}

#[derive(Default)]
pub struct EditSession {
    /// The open photo's settings being edited (one history step per gesture).
    edit: Option<EditState>,
    catalog: Option<Catalog>,
    catalog_root: Option<PathBuf>,
    saver: SidecarSaver,
    /// Photos whose damaged-sidecar warning was shown this session.
    warned: HashSet<i64>,
    damaged: Option<Damaged>,
    /// A failed sidecar write was shown; quiet until one succeeds.
    write_failed: bool,
}

impl EditSession {
    pub fn new() -> EditSession {
        EditSession::default()
    }

    /// The sidecar writer (the app flushes it when quitting).
    pub fn saver(&self) -> SidecarSaver {
        self.saver.clone()
    }

    pub fn edit(&self) -> Option<&EditState> {
        self.edit.as_ref()
    }

    pub fn edit_mut(&mut self) -> Option<&mut EditState> {
        self.edit.as_mut()
    }

    /// The catalogue the edits go to (the connection is reopened when it changes).
    pub fn set_root(&mut self, root: &Path) {
        if self.catalog_root.as_deref() != Some(root) {
            self.catalog = None;
            self.catalog_root = Some(root.to_path_buf());
        }
    }

    /// A photo opened with `settings`: a fresh edit state, with what undo and redo can do.
    pub fn start(&mut self, photo_id: i64, settings: lf_settings::EditSettings) {
        let mut edit = EditState::new(photo_id, settings);
        match connect(&mut self.catalog, self.catalog_root.as_deref()) {
            Ok(catalog) => edit.refresh_steps(catalog),
            Err(e) => log::warn!("could not open the catalogue for edits: {e}"),
        }
        self.edit = Some(edit);
    }

    /// The photo was left: its edit state and warning go.
    pub fn clear(&mut self) {
        self.edit = None;
        self.damaged = None;
    }

    /// Shows the damaged-sidecar warning for `photo_id`, the first time this session.
    pub fn warn_damaged(&mut self, photo_id: i64, name: &str, photo: &Path) {
        if self.warned.insert(photo_id) {
            self.damaged = Some(Damaged {
                photo_id,
                name: name.to_owned(),
                sidecar: lf_catalog::sidecar::path_for(photo),
            });
        }
    }

    pub fn damaged(&self) -> Option<&Damaged> {
        self.damaged.as_ref()
    }

    pub fn dismiss_damaged(&mut self) {
        self.damaged = None;
    }

    /// Runs `op` against the catalogue; true when a step was recorded (its sidecar is handed
    /// to the writer unless `sidecar_damaged`). Errors are toasted.
    pub fn run(
        &mut self,
        op: EditOp,
        photo: Option<&Path>,
        sidecar_damaged: bool,
        toasts: &mut Toasts,
    ) -> bool {
        let Some(edit) = self.edit.as_mut() else {
            return false;
        };
        let catalog = match connect(&mut self.catalog, self.catalog_root.as_deref()) {
            Ok(catalog) => catalog,
            Err(e) => {
                log::error!("could not open the catalogue for an edit: {e}");
                toasts.push(
                    Level::Error,
                    format!("The edit could not be saved in the catalogue. {e}"),
                );
                return false;
            }
        };
        let result = match op {
            EditOp::Commit => edit.commit(catalog),
            EditOp::Undo => edit.undo(catalog),
            EditOp::Redo => edit.redo(catalog),
            EditOp::Reset(groups) => edit.reset_groups(groups, catalog),
        };
        match result {
            Ok(Some(sidecar)) => {
                self.save_sidecar(sidecar, photo, sidecar_damaged);
                true
            }
            Ok(None) => false,
            Err(e) => {
                log::error!("could not record an edit ({op:?}): {e}");
                toasts.push(
                    Level::Error,
                    format!("The edit could not be saved in the catalogue. {e}"),
                );
                false
            }
        }
    }

    /// Hands a committed sidecar to the debounced writer, unless the photo's sidecar could
    /// not be read: that file is left alone until it has been fixed (the edit is in the
    /// catalogue either way).
    fn save_sidecar(&mut self, sidecar: Sidecar, photo: Option<&Path>, sidecar_damaged: bool) {
        let (Some(edit), Some(path)) = (&self.edit, photo) else {
            return;
        };
        if sidecar_damaged {
            log::debug!(
                "photo {}: its sidecar is damaged; not writing",
                edit.photo_id
            );
            return;
        }
        self.saver.push(
            PendingWrite::new(edit.photo_id, path, sidecar),
            Instant::now(),
        );
    }

    /// Puts due sidecar writes on the `Io` lane; when the next one falls due.
    pub fn pump(&self, now: Instant, jobs: &Jobs) -> Option<Instant> {
        self.saver.pump(now, jobs);
        self.saver.next_due()
    }

    /// The `Io` lane wrote the waiting sidecars.
    pub fn sidecar_written(&mut self, result: Result<(), String>, toasts: &mut Toasts) {
        match result {
            Ok(()) => self.write_failed = false,
            Err(e) => {
                if !self.write_failed {
                    toasts.push(Level::Warning, write_failed_text(&e));
                }
                self.write_failed = true;
            }
        }
    }
}
