//! Thumbnails for the grid and the filmstrip: what to ask the catalogue's pool for, and the
//! egui textures made from what it returns, kept in a least-recently-used cache.

use eframe::egui::{self, ColorImage, TextureHandle, TextureId, TextureOptions};
use image::RgbaImage;
use lf_catalog::thumbs::{Priority, Thumbnailer};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

/// How many thumbnail textures are kept.
pub const TEXTURE_CAP: usize = 600;
/// Thumbnails turned into textures per frame (the rest wait for the next frame).
pub const UPLOADS_PER_FRAME: usize = 32;

/// A least-recently-used map from photo id to `V`.
pub struct Lru<V> {
    cap: usize,
    tick: u64,
    entries: HashMap<i64, (V, u64)>,
}

impl<V> Lru<V> {
    pub fn new(cap: usize) -> Lru<V> {
        Lru {
            cap: cap.max(1),
            tick: 0,
            entries: HashMap::new(),
        }
    }

    /// The value, marked as just used.
    pub fn get(&mut self, id: i64) -> Option<&V> {
        self.tick += 1;
        let tick = self.tick;
        self.entries.get_mut(&id).map(|(v, used)| {
            *used = tick;
            &*v
        })
    }

    pub fn contains(&self, id: i64) -> bool {
        self.entries.contains_key(&id)
    }

    /// Stores `value` as just used; returns the id evicted to make room, if any.
    pub fn insert(&mut self, id: i64, value: V) -> Option<i64> {
        self.tick += 1;
        let mut evicted = None;
        if !self.entries.contains_key(&id) && self.entries.len() >= self.cap {
            evicted = self
                .entries
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(id, _)| *id);
            if let Some(old) = evicted {
                self.entries.remove(&old);
            }
        }
        self.entries.insert(id, (value, self.tick));
        evicted
    }

    pub fn remove(&mut self, id: i64) {
        self.entries.remove(&id);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Something the library should tell the user about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThumbEvent {
    /// A thumbnail worker could not start (the pool's id-0 failure).
    WorkerFailed(String),
    /// The graphics device failed; the pool has stopped and needs rebuilding.
    DeviceError(String),
}

/// The thumbnail pool and the textures made from it.
pub struct Thumbs {
    pool: Option<Thumbnailer>,
    textures: Lru<TextureHandle>,
    /// Finished thumbnails waiting to become textures.
    arrived: VecDeque<(i64, RgbaImage)>,
    /// Asked for and not back yet, with the priority asked for.
    in_flight: HashMap<i64, Priority>,
    /// Asked for once at `Normal` since the folder was shown.
    background: HashSet<i64>,
    /// Photos whose thumbnail could not be made, with the reason.
    failed: HashMap<i64, String>,
    /// The pool stopped on a device error.
    stopped: bool,
    /// The first visible cell last frame and how many were visible.
    last_range: Option<(usize, usize)>,
    /// This pool's "a worker could not start" was already reported.
    worker_failure_told: bool,
}

impl Default for Thumbs {
    fn default() -> Thumbs {
        Thumbs {
            pool: None,
            textures: Lru::new(TEXTURE_CAP),
            arrived: VecDeque::new(),
            in_flight: HashMap::new(),
            background: HashSet::new(),
            failed: HashMap::new(),
            stopped: false,
            last_range: None,
            worker_failure_told: false,
        }
    }
}

impl Thumbs {
    /// Uses `pool` from now on (at start, and after a device error).
    pub fn set_pool(&mut self, pool: Thumbnailer) {
        self.pool = Some(pool);
        self.stopped = false;
        self.in_flight.clear();
        self.background.clear();
        self.last_range = None;
        self.worker_failure_told = false;
    }

    /// The pool stopped on a device error and has not been rebuilt.
    pub fn stopped(&self) -> bool {
        self.stopped
    }

    /// Collects finished and failed thumbnails, turns a few into textures, and reports what
    /// the user should hear about. Keeps the UI waking while thumbnails are on their way.
    pub fn pump(&mut self, ctx: &egui::Context) -> Vec<ThumbEvent> {
        let mut events = Vec::new();
        if let Some(pool) = &self.pool {
            self.arrived.extend(pool.take_ready());
            let failures = pool.take_failed();
            let device = pool.take_device_error();
            events.extend(self.absorb_failures(failures));
            if let Some(message) = device {
                self.stopped = true;
                self.in_flight.clear();
                events.push(ThumbEvent::DeviceError(message));
            }
        }
        for _ in 0..UPLOADS_PER_FRAME {
            let Some((id, image)) = self.arrived.pop_front() else {
                break;
            };
            self.in_flight.remove(&id);
            self.failed.remove(&id);
            let size = [image.width() as usize, image.height() as usize];
            let pixels = ColorImage::from_rgba_unmultiplied(size, image.as_raw());
            let texture = ctx.load_texture(format!("thumb-{id}"), pixels, TextureOptions::LINEAR);
            self.textures.insert(id, texture);
        }
        if !self.arrived.is_empty() {
            ctx.request_repaint();
        } else if !self.in_flight.is_empty() && !self.stopped {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        events
    }

    /// Records the pool's failures: per photo (a badge), or id 0 (a worker could not start:
    /// one event per pool, however many workers failed).
    pub fn absorb_failures(&mut self, failures: Vec<(i64, String)>) -> Vec<ThumbEvent> {
        let mut events = Vec::new();
        for (id, message) in failures {
            if id == 0 {
                if !self.worker_failure_told {
                    self.worker_failure_told = true;
                    events.push(ThumbEvent::WorkerFailed(message));
                }
            } else {
                self.in_flight.remove(&id);
                self.failed.insert(id, message);
            }
        }
        events
    }

    /// Asks for the thumbnails of `order[first..last]` (on screen) at `High`. When the
    /// visible range jumps by more than two screens, the queued requests are dropped first,
    /// the new screen is asked for, and then the rest of the folder again at `Normal` (the
    /// pool serves `High` first, so the screen still comes first).
    pub fn visible(&mut self, order: &[i64], first: usize, last: usize) {
        let last = last.min(order.len());
        let first = first.min(last);
        let screen = (last - first).max(1);
        let jumped = self
            .last_range
            .is_some_and(|(prev, _)| prev.abs_diff(first) > 2 * screen);
        if jumped {
            if let Some(pool) = &self.pool {
                pool.cancel_pending();
            }
            self.in_flight.clear();
        }
        self.last_range = Some((first, last - first));
        self.request(&order[first..last], Priority::High);
        if jumped {
            let textures = &self.textures;
            self.background.retain(|id| textures.contains(*id));
            self.background(order);
        }
    }

    /// Asks for `ids` at `High` without the jump rule (the filmstrip's visible cells, drawn
    /// alongside the grid's).
    pub fn strip(&mut self, ids: &[i64]) {
        self.request(ids, Priority::High);
    }

    /// Asks once, at `Normal`, for every thumbnail of the folder (the pool serves fresh
    /// stored ones without decoding).
    pub fn background(&mut self, order: &[i64]) {
        let todo: Vec<i64> = order
            .iter()
            .copied()
            .filter(|id| self.background.insert(*id))
            .collect();
        self.request(&todo, Priority::Normal);
    }

    fn request(&mut self, ids: &[i64], priority: Priority) {
        let Some(pool) = self.pool.as_ref().filter(|_| !self.stopped) else {
            return;
        };
        for &id in ids {
            if self.textures.contains(id)
                || self.failed.contains_key(&id)
                || self.in_flight.get(&id).is_some_and(|p| *p >= priority)
            {
                continue;
            }
            pool.request(id, priority);
            self.in_flight.insert(id, priority);
        }
    }

    /// The photo's thumbnail texture, if made (and marks it used).
    pub fn texture(&mut self, id: i64) -> Option<(TextureId, egui::Vec2)> {
        self.textures.get(id).map(|t| (t.id(), t.size_vec2()))
    }

    /// Why the photo's thumbnail could not be made.
    pub fn error(&self, id: i64) -> Option<&str> {
        self.failed.get(&id).map(String::as_str)
    }

    /// Forgets the photo's thumbnail and failure (its file or edits changed).
    pub fn forget(&mut self, id: i64) {
        self.textures.remove(id);
        self.failed.remove(&id);
        self.background.remove(&id);
    }

    /// Another folder is shown: drop the queue and ask afresh (textures are kept).
    pub fn reset(&mut self) {
        if let Some(pool) = &self.pool {
            pool.cancel_pending();
        }
        self.in_flight.clear();
        self.background.clear();
        self.last_range = None;
    }

    /// Thumbnails asked for at `High` and not back yet.
    pub fn pending_high(&self) -> usize {
        self.in_flight
            .values()
            .filter(|p| **p == Priority::High)
            .count()
    }

    /// Thumbnails asked for and not back yet.
    pub fn pending(&self) -> usize {
        self.in_flight.len()
    }

    /// Thumbnail textures held.
    pub fn texture_count(&self) -> usize {
        self.textures.len()
    }

    /// Photos whose thumbnail could not be made.
    pub fn failed_count(&self) -> usize {
        self.failed.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lf_raw::demosaic::Quality;

    #[test]
    fn lru_evicts_the_least_recently_used() {
        let mut lru = Lru::new(3);
        assert_eq!(lru.insert(1, "a"), None);
        assert_eq!(lru.insert(2, "b"), None);
        assert_eq!(lru.insert(3, "c"), None);
        assert_eq!(lru.get(1), Some(&"a"));
        assert_eq!(lru.insert(4, "d"), Some(2), "2 is the oldest unused");
        assert!(!lru.contains(2));
        assert_eq!(lru.insert(1, "a2"), None, "replacing does not evict");
        assert_eq!(lru.len(), 3);
        assert_eq!(lru.insert(5, "e"), Some(3));
        lru.remove(4);
        assert_eq!(lru.len(), 2);
    }

    #[test]
    fn without_a_pool_nothing_is_requested() {
        let mut thumbs = Thumbs::default();
        thumbs.visible(&[1, 2, 3], 0, 3);
        thumbs.background(&[1, 2, 3]);
        assert_eq!(thumbs.pending(), 0);
        assert!(thumbs.pump(&egui::Context::default()).is_empty());
    }

    #[test]
    fn a_jump_requeues_the_folders_background_work() {
        let Some(gpu) = lf_pipeline::test_support::try_shared_gpu() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        // A paused pool keeps every request queued, so what is asked for can be counted.
        let pool = Thumbnailer::new_paused(gpu, dir.path(), 1, Quality::Fast).unwrap();
        let mut thumbs = Thumbs::default();
        thumbs.set_pool(pool);
        let order: Vec<i64> = (1..=1_000).collect();
        thumbs.background(&order);
        thumbs.visible(&order, 0, 20);
        assert_eq!(thumbs.pending(), 1_000);
        assert_eq!(thumbs.pending_high(), 20);

        // Jump far down: the queue is dropped, the new screen goes first, and the rest of
        // the folder is asked for again at Normal.
        thumbs.visible(&order, 600, 620);
        assert_eq!(thumbs.pending(), 1_000, "nothing forgotten for good");
        assert_eq!(thumbs.pending_high(), 20, "only the new screen is High");
    }

    #[test]
    fn worker_start_failures_toast_once_per_pool() {
        let mut thumbs = Thumbs::default();
        let events = thumbs.absorb_failures(vec![
            (0, "worker 1 could not open the catalogue".into()),
            (0, "worker 2 could not open the catalogue".into()),
            (7, "damaged file".into()),
        ]);
        assert_eq!(
            events,
            [ThumbEvent::WorkerFailed(
                "worker 1 could not open the catalogue".into()
            )]
        );
        assert_eq!(thumbs.error(7), Some("damaged file"));
        assert!(
            thumbs.absorb_failures(vec![(0, "again".into())]).is_empty(),
            "still the same pool"
        );
    }
}
