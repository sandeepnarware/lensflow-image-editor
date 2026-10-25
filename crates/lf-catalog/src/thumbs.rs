//! Background thumbnails: a pool of workers drains a priority queue, rendering each photo at
//! 256 px through the shared GPU and storing the JPEG in the catalogue.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use image::RgbaImage;
use lf_pipeline::Gpu;
use lf_raw::demosaic::Quality;
use lf_settings::{DevelopSettings, EditSettings};
use rusqlite::OptionalExtension;

use crate::db::{Catalog, Result};

pub const THUMB_EDGE: u32 = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    Normal,
    /// On screen right now.
    High,
}

#[derive(PartialEq, Eq)]
struct Entry {
    priority: Priority,
    seq: u64,
    id: i64,
}

impl Ord for Entry {
    fn cmp(&self, o: &Self) -> Ordering {
        // Higher priority first; within a priority, the earlier request first.
        self.priority
            .cmp(&o.priority)
            .then_with(|| o.seq.cmp(&self.seq))
    }
}
impl PartialOrd for Entry {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

#[derive(Default)]
struct State {
    heap: BinaryHeap<Entry>,
    /// The live requests; heap entries that disagree with this are stale.
    pending: HashMap<i64, Priority>,
    seq: u64,
    started: bool,
    stop: bool,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    ready: Mutex<Vec<(i64, RgbaImage)>>,
    failed: Mutex<Vec<(i64, String)>>,
}

pub struct Thumbnailer {
    shared: Arc<Shared>,
    config: Mutex<Option<(Arc<Gpu>, PathBuf, usize)>>,
    quality: Quality,
    handles: Mutex<Vec<JoinHandle<()>>>,
}

impl Thumbnailer {
    /// A sensible pool size: `min(2, available_parallelism)`. Each in-flight RAW open can use
    /// up to about 1 GB of heap at `Quality::Fast` for a 60 MP file (a quarter of that at
    /// `Preview`), so the pool is deliberately small.
    pub fn default_workers() -> usize {
        std::thread::available_parallelism().map_or(1, |n| n.get().min(2))
    }

    /// Workers start at once, each with its own catalogue connection and the shared GPU.
    pub fn new(
        gpu: Arc<Gpu>,
        catalog_path: &Path,
        workers: usize,
        quality: Quality,
    ) -> Result<Thumbnailer> {
        let t = Self::new_paused(gpu, catalog_path, workers, quality)?;
        t.start();
        Ok(t)
    }

    /// As [`Thumbnailer::new`], but nothing is processed until [`Thumbnailer::start`].
    pub fn new_paused(
        gpu: Arc<Gpu>,
        catalog_path: &Path,
        workers: usize,
        quality: Quality,
    ) -> Result<Thumbnailer> {
        Catalog::open(catalog_path)?; // fail early; also migrates
        Ok(Thumbnailer {
            shared: Arc::new(Shared {
                state: Mutex::new(State::default()),
                wake: Condvar::new(),
                ready: Mutex::new(Vec::new()),
                failed: Mutex::new(Vec::new()),
            }),
            config: Mutex::new(Some((gpu, catalog_path.to_path_buf(), workers.max(1)))),
            handles: Mutex::new(Vec::new()),
            quality,
        })
    }

    pub fn start(&self) {
        let Some((gpu, path, n)) = self.config.lock().unwrap_or_else(|e| e.into_inner()).take()
        else {
            return;
        };
        self.shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .started = true;
        let mut handles = self.handles.lock().unwrap_or_else(|e| e.into_inner());
        for _ in 0..n {
            let (shared, gpu, path, q) =
                (self.shared.clone(), gpu.clone(), path.clone(), self.quality);
            handles.push(std::thread::spawn(move || worker(shared, gpu, path, q)));
        }
        self.shared.wake.notify_all();
    }

    /// Queue a thumbnail. A photo already queued is not queued twice, but a higher priority
    /// moves it up.
    pub fn request(&self, photo_id: i64, priority: Priority) {
        let mut st = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        if st.pending.get(&photo_id).is_some_and(|p| *p >= priority) {
            return;
        }
        st.pending.insert(photo_id, priority);
        st.seq += 1;
        let seq = st.seq;
        st.heap.push(Entry {
            priority,
            seq,
            id: photo_id,
        });
        self.shared.wake.notify_one();
    }

    /// The photos whose thumbnail could not be made since the last call, with the reason.
    /// The reason is also stored in `photos.error` and logged; the grid should show a broken
    /// thumbnail and not request the photo again.
    pub fn take_failed(&self) -> Vec<(i64, String)> {
        std::mem::take(&mut *self.shared.failed.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// The thumbnails finished since the last call, in completion order.
    pub fn take_ready(&self) -> Vec<(i64, RgbaImage)> {
        std::mem::take(&mut *self.shared.ready.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

impl Drop for Thumbnailer {
    fn drop(&mut self) {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .stop = true;
        self.shared.wake.notify_all();
        for h in self
            .handles
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
        {
            let _ = h.join();
        }
    }
}

fn next_job(shared: &Shared) -> Option<i64> {
    let mut st = shared.state.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if st.stop {
            return None;
        }
        if st.started {
            while let Some(e) = st.heap.pop() {
                if st.pending.get(&e.id) == Some(&e.priority) {
                    st.pending.remove(&e.id);
                    return Some(e.id);
                }
            }
        }
        st = shared.wake.wait(st).unwrap_or_else(|e| e.into_inner());
    }
}

fn worker(shared: Arc<Shared>, gpu: Arc<Gpu>, path: PathBuf, quality: Quality) {
    let Ok(catalog) = Catalog::open(&path) else {
        return;
    };
    while let Some(id) = next_job(&shared) {
        // A decoder panic must not kill the worker (and strand the queue).
        let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            make(&catalog, &gpu, id, quality)
        }))
        .unwrap_or_else(|_| Err("the decoder crashed on this file".to_string()));
        match made {
            Ok(Some(img)) => shared
                .ready
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((id, img)),
            Ok(None) => {}
            Err(msg) => {
                let file: Option<String> = catalog
                    .conn
                    .query_row("SELECT path FROM photos WHERE id = ?1", [id], |r| r.get(0))
                    .ok();
                log::warn!(
                    "thumbnail for photo {id} ({}) failed: {msg}",
                    file.as_deref().unwrap_or("?")
                );
                if let Ok(tx) = catalog.write_tx() {
                    let _ = tx.execute(
                        "UPDATE photos SET error = ?2 WHERE id = ?1",
                        rusqlite::params![id, msg],
                    );
                    let _ = tx.commit();
                }
                shared
                    .failed
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push((id, msg));
            }
        }
    }
}

fn make(
    catalog: &Catalog,
    gpu: &Arc<Gpu>,
    id: i64,
    quality: Quality,
) -> std::result::Result<Option<RgbaImage>, String> {
    let e = |x: &dyn std::fmt::Display| x.to_string();
    let file: Option<String> = catalog
        .conn
        .query_row("SELECT path FROM photos WHERE id = ?1", [id], |r| r.get(0))
        .optional()
        .map_err(|x| e(&x))?;
    let Some(file) = file else { return Ok(None) };
    let edits: Option<(String, String)> = catalog
        .conn
        .query_row(
            "SELECT settings, updated_at FROM edits WHERE photo_id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|x| e(&x))?;
    let (settings, made_for) = match edits {
        Some((s, t)) => (EditSettings::from_json_str(&s), t),
        None => (EditSettings::default_settings(), String::new()),
    };
    let opened = lf_raw::open::open(
        Path::new(&file),
        quality,
        &DevelopSettings::from_settings(&settings),
        1024,
    )
    .map_err(|x| e(&x))?;
    let mut renderer = lf_raw::render::renderer_for(gpu, &opened).map_err(|x| e(&x))?;
    let img = renderer.pixels(&settings, THUMB_EDGE).map_err(|x| e(&x))?;
    renderer.trim();
    let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 85)
        .encode_image(&rgb)
        .map_err(|x| e(&x))?;
    let tx = catalog.write_tx().map_err(|x| e(&x))?;
    tx.execute(
        "INSERT OR REPLACE INTO thumbnails (photo_id, edge, jpeg, made_for_updated_at)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![id, THUMB_EDGE, jpeg, made_for],
    )
    .map_err(|x| e(&x))?;
    tx.commit().map_err(|x| e(&x))?;
    Ok(Some(img))
}
