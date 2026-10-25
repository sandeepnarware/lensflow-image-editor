//! Background thumbnails: a pool of workers drains a priority queue, rendering each photo at
//! 256 px through the shared GPU and storing the JPEG in the catalogue.
//!
//! A stored thumbnail is served without decoding while it is fresh: its key (the
//! `made_for_updated_at` column) is `edit:<edits.updated_at, or "-">|photo:<photos.updated_at>|
//! <file_size>|<mtime>`, taken when the render starts, so any edit (including undo, redo and
//! versions, which stamp `edits.updated_at`) or a changed file (which import records with a
//! new size, mtime and `updated_at`) makes it stale.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use image::RgbaImage;
use lf_pipeline::Gpu;
use lf_pipeline::error::PipelineError;
use lf_raw::demosaic::Quality;
use lf_settings::{DevelopSettings, EditSettings};
use rusqlite::OptionalExtension;

use crate::db::{Catalog, Result};
use crate::import::DECODER_CRASHED;
use crate::sidecar;

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
    /// The first device error (the workers stop on it), until taken.
    device_error: Mutex<Option<String>>,
    /// Set on drop: work in progress stops at the next stage.
    cancel: AtomicBool,
    /// Photos opened and rendered (stored thumbnails served fresh are not counted).
    decoded: AtomicUsize,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
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
                device_error: Mutex::new(None),
                cancel: AtomicBool::new(false),
                decoded: AtomicUsize::new(0),
            }),
            config: Mutex::new(Some((gpu, catalog_path.to_path_buf(), workers.max(1)))),
            handles: Mutex::new(Vec::new()),
            quality,
        })
    }

    pub fn start(&self) {
        let Some((gpu, path, n)) = lock(&self.config).take() else {
            return;
        };
        lock(&self.shared.state).started = true;
        let mut handles = lock(&self.handles);
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
        let mut st = lock(&self.shared.state);
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

    /// Forget every queued request (e.g. the grid scrolled to another part of the library);
    /// thumbnails already being made still finish.
    pub fn cancel_pending(&self) {
        let mut st = lock(&self.shared.state);
        st.heap.clear();
        st.pending.clear();
    }

    /// The photos whose thumbnail could not be made since the last call, with the reason.
    /// The reason is also stored in `photos.error` and logged; the grid should show a broken
    /// thumbnail and not request the photo again. Photo id `0` means a worker could not start
    /// (the reason says why; no photo is involved).
    pub fn take_failed(&self) -> Vec<(i64, String)> {
        std::mem::take(&mut *lock(&self.shared.failed))
    }

    /// The thumbnails finished since the last call, in completion order (fresh stored ones
    /// included).
    pub fn take_ready(&self) -> Vec<(i64, RgbaImage)> {
        std::mem::take(&mut *lock(&self.shared.ready))
    }

    /// A graphics device error (device lost, or an error the device reported), returned
    /// once. The workers stop on it and no photo is marked broken: the app should recreate
    /// the device and a new `Thumbnailer`.
    pub fn take_device_error(&self) -> Option<String> {
        lock(&self.shared.device_error).take()
    }

    /// How many photos this thumbnailer has decoded and rendered (stored thumbnails served
    /// fresh are not counted).
    pub fn decoded_count(&self) -> usize {
        self.shared.decoded.load(AtomicOrdering::SeqCst)
    }
}

impl Drop for Thumbnailer {
    fn drop(&mut self) {
        self.shared.cancel.store(true, AtomicOrdering::SeqCst);
        lock(&self.shared.state).stop = true;
        self.shared.wake.notify_all();
        for h in lock(&self.handles).drain(..) {
            let _ = h.join();
        }
    }
}

fn next_job(shared: &Shared) -> Option<i64> {
    let mut st = lock(&shared.state);
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

/// Why a thumbnail was not made.
enum Failure {
    /// The photo itself (unreadable, damaged, unsupported): stored and reported per photo.
    File(String),
    /// The graphics device: not the photo's fault, so nothing is stored; the workers stop.
    Device(String),
    /// The thumbnailer is being dropped.
    Cancelled,
}

fn classify(e: PipelineError) -> Failure {
    match e {
        PipelineError::DeviceLost | PipelineError::Gpu(_) => Failure::Device(e.to_string()),
        other => Failure::File(other.to_string()),
    }
}

fn worker(shared: Arc<Shared>, gpu: Arc<Gpu>, path: PathBuf, quality: Quality) {
    let catalog = match Catalog::open(&path) {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("a thumbnail worker could not open the catalogue: {e}");
            log::error!("{msg}");
            lock(&shared.failed).push((0, msg));
            return;
        }
    };
    while let Some(id) = next_job(&shared) {
        // A stored thumbnail that is still fresh needs no decode.
        match catalog.fresh_thumbnail(id) {
            Ok(Some(jpeg)) => match image::load_from_memory(&jpeg) {
                Ok(img) => {
                    lock(&shared.ready).push((id, img.to_rgba8()));
                    continue;
                }
                Err(e) => log::warn!("stored thumbnail of photo {id} is unreadable ({e})"),
            },
            Ok(None) => {}
            Err(e) => log::warn!("could not look up the stored thumbnail of photo {id}: {e}"),
        }
        // A decoder panic must not kill the worker (and strand the queue).
        let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            make(&catalog, &gpu, id, quality, &shared)
        }))
        .unwrap_or_else(|_| Err(Failure::File(DECODER_CRASHED.to_string())));
        match made {
            Ok(Some(img)) => lock(&shared.ready).push((id, img)),
            Ok(None) | Err(Failure::Cancelled) => {}
            Err(Failure::Device(msg)) => {
                log::error!("thumbnails stopped: graphics device error on photo {id}: {msg}");
                lock(&shared.device_error).get_or_insert(msg);
                lock(&shared.state).stop = true;
                shared.wake.notify_all();
                return;
            }
            Err(Failure::File(msg)) => {
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
                lock(&shared.failed).push((id, msg));
            }
        }
    }
}

/// The photo's path and the freshness key a thumbnail made now would carry.
fn freshness(catalog: &Catalog, id: i64) -> Result<Option<(String, String)>> {
    Ok(catalog
        .conn
        .query_row(
            "SELECT p.path, p.updated_at, p.file_size, p.mtime, e.updated_at
             FROM photos p LEFT JOIN edits e ON e.photo_id = p.id WHERE p.id = ?1",
            [id],
            |r| {
                let path: String = r.get(0)?;
                let photo: String = r.get(1)?;
                let size: i64 = r.get(2)?;
                let mtime: i64 = r.get(3)?;
                let edit: Option<String> = r.get(4)?;
                let edit = edit.as_deref().unwrap_or("-").to_string();
                Ok((path, format!("edit:{edit}|photo:{photo}|{size}|{mtime}")))
            },
        )
        .optional()?)
}

impl Catalog {
    /// The stored thumbnail JPEG while it is fresh (made at [`THUMB_EDGE`] for the photo's
    /// current edits and file; see the module docs), else `None`.
    pub fn fresh_thumbnail(&self, photo_id: i64) -> Result<Option<Vec<u8>>> {
        let Some((_, key)) = freshness(self, photo_id)? else {
            return Ok(None);
        };
        Ok(self
            .conn
            .query_row(
                "SELECT jpeg FROM thumbnails
                 WHERE photo_id = ?1 AND edge = ?2 AND made_for_updated_at = ?3",
                rusqlite::params![photo_id, THUMB_EDGE, key],
                |r| r.get(0),
            )
            .optional()?)
    }
}

fn make(
    catalog: &Catalog,
    gpu: &Arc<Gpu>,
    id: i64,
    quality: Quality,
    shared: &Shared,
) -> std::result::Result<Option<RgbaImage>, Failure> {
    let file_err = |x: &dyn std::fmt::Display| Failure::File(x.to_string());
    let cancelled = || shared.cancel.load(AtomicOrdering::SeqCst);
    let Some((file, made_for)) = freshness(catalog, id).map_err(|x| file_err(&x))? else {
        return Ok(None);
    };
    let settings = catalog
        .conn
        .query_row(
            "SELECT settings FROM edits WHERE photo_id = ?1",
            [id],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(|x| file_err(&x))?
        .map_or_else(EditSettings::default_settings, |s| {
            EditSettings::from_json_str(&s)
        });
    shared.decoded.fetch_add(1, AtomicOrdering::SeqCst);
    let opened = lf_raw::open::open(
        Path::new(&file),
        quality,
        &DevelopSettings::from_settings(&settings),
        1024,
    )
    .map_err(|x| file_err(&x))?;
    if cancelled() {
        return Err(Failure::Cancelled);
    }
    #[cfg(feature = "test-hooks")]
    if Path::new(&file)
        .file_stem()
        .is_some_and(|s| s == "lf-test-gpu-fail")
    {
        return Err(Failure::Device("test hook: injected device error".into()));
    }
    let mut renderer = lf_raw::render::renderer_for(gpu, &opened).map_err(classify)?;
    let img = renderer.pixels(&settings, THUMB_EDGE).map_err(classify)?;
    renderer.trim();
    drop(renderer);
    if cancelled() {
        return Err(Failure::Cancelled);
    }
    let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 85)
        .encode_image(&rgb)
        .map_err(|x| file_err(&x))?;
    // The photo is fine now: clear an earlier error, unless its sidecar is still damaged
    // (import reports that in the same column, and it stays until the sidecar is fixed).
    let sidecar_ok = sidecar::read(&sidecar::path_for(Path::new(&file))).is_ok();
    let tx = catalog.write_tx().map_err(|x| file_err(&x))?;
    tx.execute(
        "INSERT OR REPLACE INTO thumbnails (photo_id, edge, jpeg, made_for_updated_at)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![id, THUMB_EDGE, jpeg, made_for],
    )
    .map_err(|x| file_err(&x))?;
    if sidecar_ok {
        tx.execute(
            "UPDATE photos SET error = NULL WHERE id = ?1 AND error IS NOT NULL",
            [id],
        )
        .map_err(|x| file_err(&x))?;
    }
    tx.commit().map_err(|x| file_err(&x))?;
    Ok(Some(img))
}
