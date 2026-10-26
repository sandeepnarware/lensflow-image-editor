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
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long a change waits for another connection's write lock (an import batch) before it
/// is refused as busy: short, since it runs on the UI thread.
pub const BUSY_TIMEOUT: Duration = Duration::from_millis(250);
/// When a commit refused as busy is tried again.
pub const RETRY_AFTER: Duration = Duration::from_millis(50);
/// Busy refusals in a row before the user is told.
pub const BUSY_TOASTS_AFTER: u32 = 3;
/// How long leaving a photo waits for the write lock to save its last gesture: there is no
/// retry after the photo is gone, so this waits as long as the catalogue's own default.
pub const FINISH_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the open photo's edits stay quiet before the library remakes its thumbnail and
/// re-reads its row (a photo that was left is done at once).
pub const THUMB_QUIET: Duration = Duration::from_secs(1);

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

/// The toast when the open photo's last gesture could not be saved as the photo was left.
pub fn last_change_failed_text(name: &str, reason: &str) -> String {
    let stop = if reason.ends_with(['.', '!', '?']) {
        ""
    } else {
        "."
    };
    format!("Your last change to {name} could not be saved: {reason}{stop}")
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
        let opened = Catalog::open(root).map_err(|e| e.to_string())?;
        // On the UI thread: refuse quickly as busy and retry, rather than freeze.
        opened
            .set_busy_timeout(BUSY_TIMEOUT)
            .map_err(|e| e.to_string())?;
        *catalog = Some(opened);
    }
    catalog
        .as_ref()
        .ok_or_else(|| "no catalogue is open".to_owned())
}

/// Runs `f` with the [`FINISH_TIMEOUT`] wait for the write lock, then puts the quick
/// timeout back.
fn patiently<T>(
    catalog: &Catalog,
    f: impl FnOnce(&Catalog) -> Result<T, String>,
) -> Result<T, String> {
    if let Err(e) = catalog.set_busy_timeout(FINISH_TIMEOUT) {
        log::warn!("could not lengthen the catalogue's busy timeout: {e}");
    }
    let result = f(catalog);
    if let Err(e) = catalog.set_busy_timeout(BUSY_TIMEOUT) {
        log::warn!("could not restore the catalogue's busy timeout: {e}");
    }
    result
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
    /// Busy refusals in a row (an import batch held the write lock).
    busy: u32,
    /// When a commit refused as busy is tried again.
    retry_at: Option<Instant>,
    /// Photos whose edits changed in the catalogue here, with the last change's time: the
    /// library remakes their thumbnails ([`EditSession::take_edited`]).
    edited: HashMap<i64, Instant>,
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
        self.busy = 0;
        self.retry_at = None;
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
        if !matches!(&result, Err(e) if e.is_busy()) {
            self.busy = 0;
            self.retry_at = None;
        }
        match result {
            Ok(Some(sidecar)) => {
                self.save_sidecar(sidecar, photo, sidecar_damaged);
                true
            }
            Ok(None) => false,
            Err(e) if e.is_busy() => {
                // An import batch holds the write lock: the edit stays dirty and the commit
                // is tried again shortly; the user hears of it only if it keeps happening
                // (an undo or redo, which is not retried, says so at once).
                self.busy += 1;
                let dirty = self.edit.as_ref().is_some_and(|e| e.dirty);
                self.retry_at = dirty.then(|| Instant::now() + RETRY_AFTER);
                let stepping = matches!(op, EditOp::Undo | EditOp::Redo);
                // Logged once per streak: at the first refusal and when the user is told.
                if self.busy == 1 || self.busy == BUSY_TOASTS_AFTER || stepping {
                    log::info!(
                        "the catalogue was busy ({op:?}, {} in a row): {e}",
                        self.busy
                    );
                }
                if stepping || self.busy == BUSY_TOASTS_AFTER {
                    toasts.push(
                        Level::Warning,
                        format!(
                            "The catalogue is busy with another task, so the edit is not \
                             saved yet. {e}"
                        ),
                    );
                }
                false
            }
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

    /// The catalogue connection (opened on first use).
    pub fn catalog(&mut self) -> Result<&Catalog, String> {
        connect(&mut self.catalog, self.catalog_root.as_deref())
    }

    /// Runs a one-shot change `f` (a preset saved or deleted, a paste onto another photo, a
    /// version added, used or deleted) with the full [`FINISH_TIMEOUT`] wait for the write
    /// lock: unlike a gesture it is never tried again, so it waits out an import batch
    /// rather than give up after the quick timeout.
    pub fn patient<T>(
        &mut self,
        f: impl FnOnce(&Catalog) -> Result<T, String>,
    ) -> Result<T, String> {
        patiently(connect(&mut self.catalog, self.catalog_root.as_deref())?, f)
    }

    /// Hands a sidecar for a photo that is not open to the debounced writer.
    pub fn save_later(&mut self, write: PendingWrite) {
        self.saver.push(write, Instant::now());
    }

    /// Runs `f` against the catalogue and the open photo's edit state (versions, a batch
    /// reload); a sidecar it returns is handed to the writer unless `sidecar_damaged`.
    /// An error is toasted after `what`. True when `f` succeeded.
    pub fn with_catalog(
        &mut self,
        what: &str,
        photo: Option<&Path>,
        sidecar_damaged: bool,
        toasts: &mut Toasts,
        f: impl FnOnce(&Catalog, &mut EditState) -> lf_catalog::db::Result<Option<Sidecar>>,
    ) -> bool {
        let Some(edit) = self.edit.as_mut() else {
            return false;
        };
        let result = connect(&mut self.catalog, self.catalog_root.as_deref())
            .and_then(|c| patiently(c, |c| f(c, edit).map_err(|e| e.to_string())));
        match result {
            Ok(sidecar) => {
                if let Some(sidecar) = sidecar {
                    self.save_sidecar(sidecar, photo, sidecar_damaged);
                }
                true
            }
            Err(e) => {
                log::error!("{what} {e}");
                toasts.push(Level::Error, format!("{what} {e}"));
                false
            }
        }
    }

    /// Leaving the photo (another photo, Library, quitting): commits a gesture still in
    /// progress with the full [`FINISH_TIMEOUT`] wait (the quick timeout and retry are for
    /// gestures inside the open photo; once it is left there is nothing to retry). A failure
    /// is toasted with the photo's `name`. True when a step was recorded.
    pub fn finish(
        &mut self,
        photo: Option<&Path>,
        sidecar_damaged: bool,
        name: &str,
        toasts: &mut Toasts,
    ) -> bool {
        self.busy = 0;
        self.retry_at = None;
        let Some(edit) = self.edit.as_mut().filter(|e| e.dirty) else {
            return false;
        };
        let result = connect(&mut self.catalog, self.catalog_root.as_deref())
            .and_then(|c| patiently(c, |c| edit.commit(c).map_err(|e| e.to_string())));
        match result {
            Ok(Some(sidecar)) => {
                self.save_sidecar(sidecar, photo, sidecar_damaged);
                true
            }
            Ok(None) => false,
            Err(e) => {
                log::error!("the last change to {name} could not be saved: {e}");
                toasts.push(Level::Error, last_change_failed_text(name, &e));
                false
            }
        }
    }

    /// Hands a committed sidecar to the debounced writer, unless the photo's sidecar could
    /// not be read: that file is left alone until it has been fixed (the edit is in the
    /// catalogue either way).
    fn save_sidecar(&mut self, sidecar: Sidecar, photo: Option<&Path>, sidecar_damaged: bool) {
        if let Some(edit) = &self.edit {
            self.edited.insert(edit.photo_id, Instant::now());
        }
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

    /// The photos whose thumbnails are due to be remade: every photo edited here that is no
    /// longer `open`, and the open one once its edits have been quiet for [`THUMB_QUIET`].
    pub fn take_edited(&mut self, open: Option<i64>, now: Instant) -> Vec<i64> {
        let due: Vec<i64> = self
            .edited
            .iter()
            .filter(|(id, at)| Some(**id) != open || now >= **at + THUMB_QUIET)
            .map(|(id, _)| *id)
            .collect();
        for id in &due {
            self.edited.remove(id);
        }
        due
    }

    /// When the open photo's thumbnail falls due (to wake the UI then).
    pub fn edited_due(&self) -> Option<Instant> {
        self.edited.values().map(|at| *at + THUMB_QUIET).min()
    }

    /// When a commit refused as busy is tried again.
    pub fn retry_at(&self) -> Option<Instant> {
        self.retry_at
    }

    /// A commit refused as busy is due to be tried again.
    pub fn retry_due(&self, now: Instant) -> bool {
        self.retry_at().is_some_and(|at| now >= at)
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

#[cfg(test)]
mod tests {
    use super::*;
    use lf_catalog::NewPhoto;
    use lf_catalog::edits;
    use std::time::Duration;

    /// A catalogue with one photo, and a session editing it (exposure 0.5, not committed).
    fn editing() -> (tempfile::TempDir, Catalog, i64, EditSession) {
        let dir = tempfile::tempdir().unwrap();
        let c = Catalog::open(dir.path()).unwrap();
        let folder = c.add_folder("shoot").unwrap();
        let id = c
            .add_photo(
                folder,
                &NewPhoto {
                    path: "shoot/a.jpg".into(),
                    kind: "jpeg".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        let mut session = EditSession::new();
        session.set_root(dir.path());
        session.start(id, lf_settings::EditSettings::default_settings());
        session.edit_mut().unwrap().set(|s| s.exposure = 0.5);
        (dir, c, id, session)
    }

    #[test]
    fn leaving_the_photo_waits_out_a_busy_catalogue_and_saves_the_edit() {
        let (dir, c, id, mut session) = editing();
        let db = dir.path().join(".lensflow/catalog.sqlite");
        let (locked_tx, locked) = std::sync::mpsc::channel();
        // An import batch holds the write lock for 600 ms, longer than the quick timeout.
        let holder = std::thread::spawn(move || {
            let other = rusqlite::Connection::open(db).unwrap();
            other.execute_batch("BEGIN IMMEDIATE").unwrap();
            locked_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(600));
            other.execute_batch("ROLLBACK").unwrap();
        });
        locked.recv().unwrap();
        let mut toasts = Toasts::default();
        assert!(session.finish(None, false, "a.jpg", &mut toasts), "saved");
        holder.join().unwrap();
        assert_eq!(toasts.count(), 0);
        assert!(!session.edit().unwrap().dirty);
        assert_eq!(edits::get_settings(&c, id).unwrap().unwrap().exposure, 0.5);
        // The quick timeout is back for gestures inside the photo.
        let other =
            rusqlite::Connection::open(dir.path().join(".lensflow/catalog.sqlite")).unwrap();
        other.execute_batch("BEGIN IMMEDIATE").unwrap();
        session.edit_mut().unwrap().set(|s| s.exposure = 0.7);
        let began = Instant::now();
        assert!(!session.run(EditOp::Commit, None, false, &mut toasts));
        assert!(began.elapsed() < Duration::from_secs(2));
        other.execute_batch("ROLLBACK").unwrap();
    }

    #[test]
    fn a_one_shot_change_waits_out_a_busy_catalogue() {
        let (dir, c, _id, mut session) = editing();
        let db = dir.path().join(".lensflow/catalog.sqlite");
        let (locked_tx, locked) = std::sync::mpsc::channel();
        // An import batch holds the write lock for 600 ms, longer than the quick timeout.
        let holder = std::thread::spawn(move || {
            let other = rusqlite::Connection::open(db).unwrap();
            other.execute_batch("BEGIN IMMEDIATE").unwrap();
            locked_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(600));
            other.execute_batch("ROLLBACK").unwrap();
        });
        locked.recv().unwrap();
        let saved = session.patient(|c| {
            lf_catalog::presets::save(c, 7, "Warm", &serde_json::json!({"v": 2}))
                .map_err(|e| e.to_string())
        });
        holder.join().unwrap();
        assert!(saved.is_ok(), "{saved:?}");
        assert_eq!(lf_catalog::presets::list(&c, 7).unwrap().len(), 1);
    }

    #[test]
    fn a_last_change_that_cannot_be_saved_says_so() {
        let (_dir, c, id, mut session) = editing();
        // The photo left the catalogue (its folder was removed) under the open edit.
        c.remove_folder(c.folders().unwrap()[0].id).unwrap();
        let mut toasts = Toasts::default();
        assert!(!session.finish(None, false, "a.jpg", &mut toasts));
        assert_eq!(toasts.count(), 1);
        let text = toasts.last_text().unwrap();
        assert!(
            text.starts_with("Your last change to a.jpg could not be saved: "),
            "{text}"
        );
        assert!(text.ends_with('.'), "{text}");
        assert_eq!(
            last_change_failed_text("a.jpg", "disk full"),
            "Your last change to a.jpg could not be saved: disk full."
        );
        let _ = id;
    }

    #[test]
    fn a_busy_catalogue_keeps_the_edit_and_retries_quietly() {
        let dir = tempfile::tempdir().unwrap();
        let c = Catalog::open(dir.path()).unwrap();
        let folder = c.add_folder("shoot").unwrap();
        let id = c
            .add_photo(
                folder,
                &NewPhoto {
                    path: "shoot/a.jpg".into(),
                    kind: "jpeg".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        let mut session = EditSession::new();
        session.set_root(dir.path());
        session.start(id, lf_settings::EditSettings::default_settings());
        session.edit_mut().unwrap().set(|s| s.exposure = 0.5);

        // Another connection (an import batch) holds the write lock.
        let other =
            rusqlite::Connection::open(dir.path().join(".lensflow/catalog.sqlite")).unwrap();
        other.execute_batch("BEGIN IMMEDIATE").unwrap();
        let mut toasts = Toasts::default();
        for attempt in 1..=BUSY_TOASTS_AFTER {
            let began = Instant::now();
            assert!(!session.run(EditOp::Commit, None, false, &mut toasts));
            assert!(
                began.elapsed() < Duration::from_secs(2),
                "the UI waits a short busy timeout, not ten seconds"
            );
            assert!(session.edit().unwrap().dirty, "the edit is kept");
            let at = session.retry_at().expect("a retry is scheduled");
            assert!(session.retry_due(at) && !session.retry_due(at - RETRY_AFTER));
            let expected = usize::from(attempt == BUSY_TOASTS_AFTER);
            assert_eq!(toasts.count(), expected, "attempt {attempt}");
        }

        // The lock goes: the retry records the edit, once.
        other.execute_batch("ROLLBACK").unwrap();
        assert!(session.run(EditOp::Commit, None, false, &mut toasts));
        assert!(!session.edit().unwrap().dirty);
        assert!(session.retry_at().is_none());
        assert_eq!(edits::history_len(&c, id).unwrap(), 2);
    }
}
