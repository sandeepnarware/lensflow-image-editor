//! The open photo's settings while they are edited, and the commit model: the panels change
//! `settings` on every frame of a drag (the preview follows), and a gesture's end (slider
//! release, curve point release, toggle click) commits it once, so one drag is one history
//! step in the catalogue.

use lf_catalog::edits;
use lf_catalog::{Catalog, Sidecar};
use lf_settings::EditSettings;
use lf_settings::groups::{GroupKey, apply_groups};

pub type Result<T> = std::result::Result<T, lf_catalog::CatalogError>;

#[derive(Debug, Clone)]
pub struct EditState {
    pub photo_id: i64,
    /// What the preview shows (changes live while dragging).
    pub settings: EditSettings,
    /// What the catalogue holds.
    pub committed: EditSettings,
    /// `settings` differs from `committed` (a gesture is in progress).
    pub dirty: bool,
    /// What undo and redo can do, from the catalogue's history.
    steps: (i64, i64),
}

impl EditState {
    pub fn new(photo_id: i64, settings: EditSettings) -> EditState {
        EditState {
            photo_id,
            committed: settings.clone(),
            settings,
            dirty: false,
            steps: (0, 0),
        }
    }

    /// Changes the live settings (the caller redraws).
    pub fn set(&mut self, f: impl FnOnce(&mut EditSettings)) {
        f(&mut self.settings);
        self.dirty = self.settings != self.committed;
    }

    /// Ends a gesture: records the settings in the catalogue (one history step) when they
    /// changed, and returns the sidecar to write. Nothing is recorded when they did not.
    pub fn commit(&mut self, catalog: &Catalog) -> Result<Option<Sidecar>> {
        if self.settings == self.committed {
            self.dirty = false;
            return Ok(None);
        }
        let sidecar = edits::set_settings(catalog, self.photo_id, &self.settings)?;
        self.committed = self.settings.clone();
        self.dirty = false;
        self.refresh_steps(catalog);
        Ok(Some(sidecar))
    }

    /// Steps back through the catalogue's history (a gesture in progress is committed
    /// first, so it is the step undone). `None` at the oldest step.
    pub fn undo(&mut self, catalog: &Catalog) -> Result<Option<Sidecar>> {
        self.step(catalog, true)
    }

    /// Steps forward again. `None` at the newest step.
    pub fn redo(&mut self, catalog: &Catalog) -> Result<Option<Sidecar>> {
        self.step(catalog, false)
    }

    fn step(&mut self, catalog: &Catalog, back: bool) -> Result<Option<Sidecar>> {
        self.commit(catalog)?;
        let stepped = if back {
            edits::undo(catalog, self.photo_id)?
        } else {
            edits::redo(catalog, self.photo_id)?
        };
        if let Some(sidecar) = &stepped {
            self.settings = sidecar.settings.clone();
            self.committed = sidecar.settings.clone();
            self.dirty = false;
        }
        self.refresh_steps(catalog);
        Ok(stepped)
    }

    /// Takes `settings` as what the catalogue now holds (a version used, a batch applied).
    pub fn adopt(&mut self, settings: EditSettings, catalog: &Catalog) {
        self.settings = settings.clone();
        self.committed = settings;
        self.dirty = false;
        self.refresh_steps(catalog);
    }

    /// Puts `groups` back to their defaults, as one history step.
    pub fn reset_groups(
        &mut self,
        groups: &[GroupKey],
        catalog: &Catalog,
    ) -> Result<Option<Sidecar>> {
        let defaults = EditSettings::default_settings().to_value();
        self.settings = apply_groups(&self.settings, &defaults, groups);
        self.commit(catalog)
    }

    /// Reads what undo and redo can do from the catalogue (after opening the photo).
    pub fn refresh_steps(&mut self, catalog: &Catalog) {
        match edits::history_steps(catalog, self.photo_id) {
            Ok(steps) => self.steps = steps,
            Err(e) => log::warn!("could not read the history of photo {}: {e}", self.photo_id),
        }
    }

    pub fn can_undo(&self) -> bool {
        self.dirty || self.steps.0 > 0
    }

    pub fn can_redo(&self) -> bool {
        !self.dirty && self.steps.1 > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lf_catalog::NewPhoto;

    fn catalogue() -> (tempfile::TempDir, Catalog, i64) {
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
        (dir, c, id)
    }

    #[test]
    fn commit_only_when_changed() {
        let (_dir, c, id) = catalogue();
        let mut edit = EditState::new(id, EditSettings::default_settings());
        assert!(edit.commit(&c).unwrap().is_none(), "nothing changed");
        assert_eq!(edits::history_len(&c, id).unwrap(), 0);

        // One drag: many frames of changes, one release.
        for frame in 1..=30 {
            edit.set(|s| s.exposure = f64::from(frame) * 0.05);
            assert!(edit.dirty);
        }
        let sidecar = edit.commit(&c).unwrap().expect("a step was recorded");
        assert!((sidecar.settings.exposure - 1.5).abs() < 1e-9);
        assert!(!edit.dirty);
        // The seeded "before" step and the drag: one step for the whole gesture.
        assert_eq!(edits::history_len(&c, id).unwrap(), 2);
        assert!(
            edit.commit(&c).unwrap().is_none(),
            "a second release records nothing"
        );
        // A drag that ends where it began records nothing either.
        edit.set(|s| s.exposure = 0.3);
        edit.set(|s| s.exposure = 1.5);
        assert!(!edit.dirty);
        assert!(edit.commit(&c).unwrap().is_none());
        assert_eq!(edits::history_len(&c, id).unwrap(), 2);
        assert!(edit.can_undo() && !edit.can_redo());
    }

    #[test]
    fn undo_then_redo_restores() {
        let (_dir, c, id) = catalogue();
        let mut edit = EditState::new(id, EditSettings::default_settings());
        edit.set(|s| s.exposure = 0.5);
        edit.commit(&c).unwrap();
        edit.set(|s| s.contrast = 20.0);
        edit.commit(&c).unwrap();

        let back = edit.undo(&c).unwrap().expect("undone");
        assert_eq!(back.settings.contrast, 0.0);
        assert_eq!((edit.settings.exposure, edit.settings.contrast), (0.5, 0.0));
        assert_eq!(edit.settings, edit.committed);
        assert!(edit.can_undo() && edit.can_redo());

        edit.redo(&c).unwrap().expect("redone");
        assert_eq!(
            (edit.settings.exposure, edit.settings.contrast),
            (0.5, 20.0)
        );
        assert!(!edit.can_redo());
        assert!(edit.redo(&c).unwrap().is_none(), "nothing to redo");

        // Undo during a gesture undoes that gesture.
        edit.set(|s| s.whites = 40.0);
        edit.undo(&c).unwrap().expect("undone");
        assert_eq!(edit.settings.whites, 0.0);
        assert_eq!(edit.settings.contrast, 20.0);

        // Reset puts a group back as one step, which undo takes back.
        edit.reset_groups(&[GroupKey::Light], &c)
            .unwrap()
            .expect("reset");
        assert_eq!((edit.settings.exposure, edit.settings.contrast), (0.0, 0.0));
        edit.undo(&c).unwrap();
        assert_eq!(
            (edit.settings.exposure, edit.settings.contrast),
            (0.5, 20.0)
        );
    }
}
