//! Sidecar writes are debounced: a drag commits once on release, but a run of quick edits
//! (keys, clicks) still writes the file beside the photo once, a second after the last one.
//! The catalogue already holds every edit, so a write that never happens loses nothing but
//! the copy beside the photo; quitting flushes it synchronously (Review Focus 4).

use crate::jobs::{Jobs, Lane, Msg};
use lf_catalog::{Sidecar, sidecar};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// How long the edits must be still before the sidecar is written.
pub const QUIET: Duration = Duration::from_secs(1);

/// Holds the latest value until it has been left alone for [`QUIET`].
#[derive(Debug)]
pub struct Debouncer<T> {
    pending: Option<(T, Instant)>,
}

impl<T> Default for Debouncer<T> {
    fn default() -> Self {
        Debouncer { pending: None }
    }
}

impl<T> Debouncer<T> {
    /// Replaces the waiting value (the newest wins) and restarts the quiet period.
    pub fn push(&mut self, value: T, now: Instant) {
        self.pending = Some((value, now));
    }

    /// The waiting value, once nothing was pushed for [`QUIET`].
    pub fn due(&mut self, now: Instant) -> Option<T> {
        match &self.pending {
            Some((_, at)) if now.saturating_duration_since(*at) >= QUIET => self.flush(),
            _ => None,
        }
    }

    /// The waiting value now, whatever the time.
    pub fn flush(&mut self) -> Option<T> {
        self.pending.take().map(|(value, _)| value)
    }

    /// When the waiting value falls due.
    pub fn next_due(&self) -> Option<Instant> {
        self.pending.as_ref().map(|(_, at)| *at + QUIET)
    }

    pub fn peek(&self) -> Option<&T> {
        self.pending.as_ref().map(|(value, _)| value)
    }
}

/// One sidecar to write beside its photo.
#[derive(Debug, Clone)]
pub struct PendingWrite {
    pub photo_id: i64,
    /// `<photo>.lensflow.json`.
    pub path: PathBuf,
    pub sidecar: Sidecar,
}

impl PendingWrite {
    pub fn new(photo_id: i64, photo: &Path, sidecar: Sidecar) -> PendingWrite {
        PendingWrite {
            photo_id,
            path: sidecar::path_for(photo),
            sidecar,
        }
    }

    fn write(&self) -> Result<(), String> {
        sidecar::write(&self.path, &self.sidecar).map_err(|e| {
            log::warn!("could not write {}: {e}", self.path.display());
            e.to_string()
        })
    }
}

#[derive(Default)]
struct Shared {
    debouncer: Debouncer<PendingWrite>,
    /// Due (or overtaken by another photo's edit) and waiting for the `Io` lane.
    queued: VecDeque<PendingWrite>,
    /// A writer job is on the `Io` lane and has not started yet.
    job_waiting: bool,
}

/// The app's sidecar writer: the debouncer behind a lock, so the UI pushes, the `Io` lane
/// writes, and `on_exit` (or the device-reset dialog's thread, when the window is gone)
/// flushes synchronously. Clones share the same state.
///
/// Writes run under a second lock, and each writer takes its values only once it holds that
/// lock, so a newer sidecar is never overwritten by an older one finishing late.
#[derive(Clone, Default)]
pub struct SidecarSaver {
    shared: Arc<Mutex<Shared>>,
    writing: Arc<Mutex<()>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl SidecarSaver {
    pub fn new() -> SidecarSaver {
        SidecarSaver::default()
    }

    /// Waits a quiet second before writing `write`. A different photo's waiting write is
    /// queued at once (it is not delayed by, or lost to, this photo's edits).
    pub fn push(&self, write: PendingWrite, now: Instant) {
        let mut shared = lock(&self.shared);
        if shared
            .debouncer
            .peek()
            .is_some_and(|w| w.photo_id != write.photo_id)
            && let Some(earlier) = shared.debouncer.flush()
        {
            shared.queued.push_back(earlier);
        }
        shared.debouncer.push(write, now);
    }

    /// Moves a due write to the queue; true when a writer job should be spawned (something
    /// is queued and no writer is already waiting on the lane).
    pub fn tick(&self, now: Instant) -> bool {
        let mut shared = lock(&self.shared);
        if let Some(due) = shared.debouncer.due(now) {
            shared.queued.push_back(due);
        }
        if shared.queued.is_empty() || shared.job_waiting {
            return false;
        }
        shared.job_waiting = true;
        true
    }

    /// When the waiting write falls due (to wake the UI then).
    pub fn next_due(&self) -> Option<Instant> {
        lock(&self.shared).debouncer.next_due()
    }

    /// Something is waiting or queued.
    pub fn has_pending(&self) -> bool {
        let shared = lock(&self.shared);
        shared.debouncer.peek().is_some() || !shared.queued.is_empty()
    }

    /// Runs [`SidecarSaver::tick`] and, when it says so, puts a writer on the `Io` lane.
    pub fn pump(&self, now: Instant, jobs: &Jobs) {
        if self.tick(now) {
            let saver = self.clone();
            jobs.spawn(Lane::Io, "write sidecar", move || {
                Msg::SidecarWritten(saver.write_queued())
            });
        }
    }

    /// The `Io` lane's job: writes everything queued, oldest first. The first error is
    /// returned (each is logged).
    pub fn write_queued(&self) -> Result<(), String> {
        let _writing = lock(&self.writing);
        let queued: Vec<PendingWrite> = {
            let mut shared = lock(&self.shared);
            shared.job_waiting = false;
            shared.queued.drain(..).collect()
        };
        first_error(queued.iter().map(PendingWrite::write))
    }

    /// Writes `write` now, on this thread (a batch on the `Io` lane): any older write for
    /// the same photo still waiting or queued is dropped first, under the write lock, so it
    /// can never land after this one.
    pub fn write_now(&self, write: PendingWrite) -> Result<(), String> {
        let _writing = lock(&self.writing);
        {
            let mut shared = lock(&self.shared);
            shared.queued.retain(|w| w.photo_id != write.photo_id);
            if shared
                .debouncer
                .peek()
                .is_some_and(|w| w.photo_id == write.photo_id)
            {
                shared.debouncer.flush();
            }
        }
        write.write()
    }

    /// Writes everything queued and waiting, now, on this thread (quitting).
    pub fn flush(&self) -> Result<(), String> {
        let _writing = lock(&self.writing);
        let all: Vec<PendingWrite> = {
            let mut shared = lock(&self.shared);
            let mut all: Vec<PendingWrite> = shared.queued.drain(..).collect();
            all.extend(shared.debouncer.flush());
            all
        };
        if !all.is_empty() {
            log::info!("writing {} edit file(s) now", all.len());
        }
        first_error(all.iter().map(PendingWrite::write))
    }
}

fn first_error(results: impl Iterator<Item = Result<(), String>>) -> Result<(), String> {
    let mut first = Ok(());
    for result in results {
        if first.is_ok() && result.is_err() {
            first = result;
        }
    }
    first
}

#[cfg(test)]
mod tests {
    use super::*;
    use lf_settings::EditSettings;

    /// A sidecar holding `exposure` (read back from JSON: lf-app has no clock types).
    pub(crate) fn sidecar(exposure: f64) -> Sidecar {
        let mut settings = EditSettings::default_settings();
        settings.exposure = exposure;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.lensflow.json");
        let json = serde_json::json!({ "v": 2, "settings": settings.to_value() });
        std::fs::write(&path, json.to_string()).unwrap();
        sidecar::read(&path).unwrap().unwrap()
    }

    #[test]
    fn debouncer_fires_once_after_quiet_second_and_flushes() {
        let t0 = Instant::now();
        let mut d = Debouncer::default();
        assert_eq!(d.due(t0), None);
        d.push(1, t0);
        d.push(2, t0 + Duration::from_millis(400));
        assert_eq!(
            d.due(t0 + Duration::from_millis(1000)),
            None,
            "not quiet yet"
        );
        assert_eq!(
            d.next_due(),
            Some(t0 + Duration::from_millis(1400)),
            "the quiet second restarts on each push"
        );
        assert_eq!(
            d.due(t0 + Duration::from_millis(1400)),
            Some(2),
            "newest wins"
        );
        assert_eq!(d.due(t0 + Duration::from_secs(10)), None, "fires once");

        d.push(3, t0 + Duration::from_secs(20));
        assert_eq!(d.flush(), Some(3), "flush does not wait");
        assert_eq!(d.flush(), None);
        assert_eq!(d.due(t0 + Duration::from_secs(60)), None);
    }

    #[test]
    fn saver_flush_writes_waiting_sidecars_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.jpg"), dir.path().join("b.jpg"));
        let saver = SidecarSaver::new();
        let t0 = Instant::now();
        saver.push(PendingWrite::new(1, &a, sidecar(0.1)), t0);
        saver.push(PendingWrite::new(1, &a, sidecar(0.2)), t0);
        // Another photo's edit queues the first photo's write straight away.
        saver.push(PendingWrite::new(2, &b, sidecar(0.3)), t0);
        assert!(saver.tick(t0), "photo 1's write is queued");
        assert!(!saver.tick(t0), "one writer job at a time");
        assert!(!sidecar::path_for(&a).exists());

        // Quitting (Review Focus 4): everything is written now, the newest of each photo.
        saver.flush().unwrap();
        let read = |p: &Path| {
            sidecar::read(&sidecar::path_for(p))
                .unwrap()
                .unwrap()
                .settings
                .exposure
        };
        assert_eq!(read(&a), 0.2);
        assert_eq!(read(&b), 0.3);
        assert!(!saver.has_pending());
        // The writer job that was queued finds nothing left to do.
        saver.write_queued().unwrap();
    }

    #[test]
    fn saver_writes_on_the_lane_once_due() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.jpg");
        let saver = SidecarSaver::new();
        let t0 = Instant::now();
        saver.push(PendingWrite::new(1, &a, sidecar(0.5)), t0);
        assert!(!saver.tick(t0 + Duration::from_millis(900)));
        assert!(saver.tick(t0 + QUIET));
        saver.write_queued().unwrap();
        assert!(sidecar::path_for(&a).exists());
        assert!(!saver.tick(t0 + QUIET * 3), "nothing left");
    }
}
