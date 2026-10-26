//! Folder import: find candidates, then record them in the catalogue in resumable batches.
//! Only headers and EXIF are read; a file whose headers lack a size is decoded small. Each
//! new or changed file's sidecar is read and reconciled in the same transaction, so edits and
//! versions made elsewhere are adopted before anything can overwrite them.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use lf_raw::RawError;
use lf_raw::demosaic::Quality;
use lf_raw::open::{Kind, summary};
use lf_settings::DevelopSettings;

use crate::db::{Catalog, CatalogError};
use crate::edits::reconcile_in;
use crate::model::{now, ts};
use crate::sidecar::{self, Sidecar};

const EXTENSIONS: &[&str] = &[
    // RAW (the formats in tests/raw-samples/manifest.json, and the other common ones)
    "dng", "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "raf", "orf", "rw2", "pef",
    "srw", "3fr", "iiq", "x3f", "dcr", "kdc", "mrw", "erf", "mef", "mos", // pictures
    "jpg", "jpeg", "png", "tif", "tiff", "webp",
];

/// `progress` is called at least this often (in files) while a batch is read.
const PROGRESS_EVERY: usize = 25;

#[derive(Clone, Debug)]
pub struct Candidate {
    pub path: PathBuf,
    pub size: u64,
    /// Milliseconds since the Unix epoch.
    pub mtime: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub added: usize,
    pub updated: usize,
    pub skipped: usize,
}

/// Returned by a progress callback to stop the import (after the batch it reported).
#[derive(Clone, Copy, Debug)]
pub struct ImportAbort;

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    #[error("import stopped")]
    Aborted,
}

impl From<rusqlite::Error> for ImportError {
    fn from(e: rusqlite::Error) -> Self {
        ImportError::Catalog(e.into())
    }
}

fn supported(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name.ends_with(".lensflow.json") {
        return false;
    }
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// What [`scan`] found under a folder.
#[derive(Clone, Debug, Default)]
pub struct Scan {
    /// The supported files, sorted by path.
    pub candidates: Vec<Candidate>,
    /// The folder itself could be listed. False for a folder that is gone or on a drive that
    /// is unplugged or unreachable: [`mark_missing`] then marks nothing.
    pub root_readable: bool,
}

/// Every supported file under `root`, recursively, skipping hidden entries, the `.lensflow`
/// folder and `*.lensflow.json` sidecars. Unreadable directories below the root are skipped;
/// whether the root itself was readable is reported in [`Scan::root_readable`].
pub fn scan(root: &Path) -> Scan {
    let root_readable = fs::read_dir(root).is_ok();
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    let mut visited = std::collections::HashSet::new();
    while let Some(dir) = stack.pop() {
        // Symlinks are followed, so remember real directories to avoid loops.
        if !fs::canonicalize(&dir).is_ok_and(|c| visited.insert(c)) {
            continue;
        }
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            let Ok(meta) = fs::metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() && supported(&path) {
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_millis() as i64);
                out.push(Candidate {
                    path,
                    size: meta.len(),
                    mtime,
                });
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Scan {
        candidates: out,
        root_readable,
    }
}

struct Meta {
    kind: String,
    width: Option<u32>,
    height: Option<u32>,
    camera: Option<String>,
    lens: Option<String>,
    captured_at: Option<String>,
    orientation: i64,
    error: Option<String>,
}

fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Raw => "raw",
        Kind::Jpeg => "jpeg",
        Kind::Png => "png",
        Kind::Tiff => "tiff",
        Kind::WebP => "webp",
        Kind::Heic => "heic",
        Kind::Unknown => "unknown",
    }
}

fn read_meta(path: &Path) -> Result<Meta, RawError> {
    #[cfg(feature = "test-hooks")]
    if path.file_stem().is_some_and(|s| s == "lf-test-panic") {
        panic!("test hook: the decoder panics on {}", path.display());
    }
    let (kind, exif, size) = summary(path)?;
    let size = match size {
        Some(s) => Some(s),
        None => lf_raw::open::open(path, Quality::Fast, &DevelopSettings::default(), 1024)
            .ok()
            .map(|o| (o.full_width, o.full_height)),
    };
    let camera = match (exif.make.as_deref(), exif.model.as_deref()) {
        (Some(make), Some(model)) if model.starts_with(make) => Some(model.to_string()),
        (Some(make), Some(model)) => Some(format!("{make} {model}")),
        (Some(x), None) | (None, Some(x)) => Some(x.to_string()),
        (None, None) => None,
    };
    Ok(Meta {
        kind: kind_name(kind).to_string(),
        width: size.map(|s| s.0),
        height: size.map(|s| s.1),
        camera,
        lens: exif.lens,
        captured_at: exif.captured_at,
        orientation: i64::from(exif.orientation.max(1)),
        error: None,
    })
}

/// [`read_meta`], with any decoder panic caught: one bad file must not stop an import.
fn read_meta_guarded(path: &Path) -> Meta {
    match std::panic::catch_unwind(|| read_meta(path)) {
        Ok(Ok(m)) => m,
        Ok(Err(e)) => unreadable(e.to_string()),
        Err(_) => {
            log::error!("the decoder crashed reading {}", path.display());
            unreadable(DECODER_CRASHED.to_string())
        }
    }
}

/// The reason stored for a file whose decoder panicked.
pub(crate) const DECODER_CRASHED: &str = "the decoder crashed on this file";

fn unreadable(error: String) -> Meta {
    Meta {
        kind: "unreadable".into(),
        width: None,
        height: None,
        camera: None,
        lens: None,
        captured_at: None,
        orientation: 1,
        error: Some(error),
    }
}

/// One file of a batch to record: the candidate, its row if catalogued, its facts and its
/// sidecar as read.
struct Work<'a> {
    key: String,
    candidate: &'a Candidate,
    id: Option<i64>,
    meta: Meta,
    sidecar: Result<Option<Sidecar>, CatalogError>,
}

/// Import `candidates` into `folder_id`, `batch` files per transaction. After each batch has
/// committed, and every 25 files within a batch, `progress` is called; an `Err` stops the import,
/// keeping what was committed (an in-progress batch is dropped).
/// Files already catalogued with the same size and mtime are skipped, so a rerun resumes.
///
/// For each new or changed file the sidecar beside it is read and reconciled
/// ([`crate::edits::reconcile`]) in the batch's transaction: a sidecar's settings, versions and
/// unknown keys are adopted, and a sidecar the catalogue supersedes is rewritten after the
/// commit. A sidecar that cannot be read is reported in `photos.sidecar_error` (apart from
/// `photos.error`, the file's own problem) and left untouched. A decoder panic marks the file
/// unreadable ("the decoder crashed on this file"). Every file seen has its `missing` flag
/// cleared.
pub fn run(
    catalog: &Catalog,
    folder_id: i64,
    candidates: &[Candidate],
    batch: usize,
    progress: &mut dyn FnMut(Progress) -> Result<(), ImportAbort>,
) -> Result<Summary, ImportError> {
    let total = candidates.len();
    let mut summary = Summary::default();
    let mut done = 0;
    for chunk in candidates.chunks(batch.max(1)) {
        let mut existing: HashMap<String, (i64, i64, i64, bool)> = HashMap::new();
        {
            let mut st = catalog
                .conn
                .prepare("SELECT id, file_size, mtime, missing FROM photos WHERE path = ?1")?;
            for c in chunk {
                let key = c.path.to_string_lossy().into_owned();
                let mut rows = st.query([&key])?;
                if let Some(r) = rows.next()? {
                    existing.insert(key, (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?));
                }
            }
        }
        let mut work = Vec::new();
        let mut seen_again = Vec::new();
        for (i, c) in chunk.iter().enumerate() {
            let key = c.path.to_string_lossy().into_owned();
            match existing.get(&key) {
                Some(&(id, size, mtime, missing)) if size == c.size as i64 && mtime == c.mtime => {
                    summary.skipped += 1;
                    if missing {
                        seen_again.push(id);
                    }
                }
                found => work.push(Work {
                    id: found.map(|f| f.0),
                    meta: read_meta_guarded(&c.path),
                    sidecar: sidecar::read(&sidecar::path_for(&c.path)),
                    candidate: c,
                    key,
                }),
            }
            // Movement for the UI inside a big batch. Stopping here discards the batch in
            // progress (nothing of it is committed yet); the final call of a batch is made
            // after its commit.
            let n = i + 1;
            if n % PROGRESS_EVERY == 0
                && n < chunk.len()
                && progress(Progress {
                    done: done + n,
                    total,
                })
                .is_err()
            {
                return Err(ImportError::Aborted);
            }
        }
        let mut to_write: Vec<(PathBuf, Sidecar)> = Vec::new();
        if !work.is_empty() || !seen_again.is_empty() {
            let tx = catalog.write_tx()?;
            let t = ts(now());
            for id in &seen_again {
                tx.execute("UPDATE photos SET missing = 0 WHERE id = ?1", [id])?;
            }
            for w in &work {
                let (c, m) = (w.candidate, &w.meta);
                let id = match w.id {
                    Some(id) => {
                        tx.execute(
                            "UPDATE photos SET file_size=?2, mtime=?3, width=?4, height=?5, camera=?6,
                             lens=?7, captured_at=?8, kind=?9, orientation=?10, error=?11, updated_at=?12,
                             missing=0
                             WHERE id=?1",
                            rusqlite::params![
                                id,
                                c.size as i64,
                                c.mtime,
                                m.width,
                                m.height,
                                m.camera,
                                m.lens,
                                m.captured_at,
                                m.kind,
                                m.orientation,
                                m.error,
                                t
                            ],
                        )?;
                        summary.updated += 1;
                        id
                    }
                    None => {
                        tx.execute(
                            "INSERT INTO photos (folder_id, path, file_size, mtime, width, height,
                             camera, lens, captured_at, kind, orientation, error, imported_at, updated_at)
                             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?13)",
                            rusqlite::params![
                                folder_id,
                                w.key,
                                c.size as i64,
                                c.mtime,
                                m.width,
                                m.height,
                                m.camera,
                                m.lens,
                                m.captured_at,
                                m.kind,
                                m.orientation,
                                m.error,
                                t
                            ],
                        )?;
                        summary.added += 1;
                        tx.last_insert_rowid()
                    }
                };
                match &w.sidecar {
                    Ok(sc) => {
                        let r = reconcile_in(&tx, id, sc.as_ref())?;
                        if let Some(out) = r.sidecar_to_write {
                            to_write.push((sidecar::path_for(&c.path), out));
                        }
                    }
                    Err(e) => {
                        log::warn!("the sidecar of {} could not be read: {e}", c.path.display());
                        tx.execute(
                            "UPDATE photos SET sidecar_error = ?2 WHERE id = ?1",
                            rusqlite::params![id, sidecar::damaged_message(&c.path, e)],
                        )?;
                    }
                }
            }
            tx.commit()?;
        }
        for (path, sc) in &to_write {
            if let Err(e) = sidecar::write(path, sc) {
                log::warn!("could not update the sidecar {}: {e}", path.display());
            }
        }
        done += chunk.len();
        if progress(Progress { done, total }).is_err() {
            return Err(ImportError::Aborted);
        }
    }
    Ok(summary)
}

/// Flag the photos catalogued under `folder_id` whose file is not among `scanned` (the folder's
/// fresh [`scan`]) as missing (`photos.missing = 1`); [`run`] clears the flag when the file is
/// seen again. Returns how many rows this call newly flagged.
///
/// A scan that did not reach the folder marks nothing: one whose root could not be read (a
/// drive unplugged or a network share offline), and one that found no files at all (the empty
/// mount point an unplugged drive leaves behind is indistinguishable from a folder emptied on
/// purpose, and wrongly hiding every photo is the worse mistake; ask, then call
/// [`mark_missing_confirmed`]).
pub fn mark_missing(
    catalog: &Catalog,
    folder_id: i64,
    scanned: &Scan,
) -> Result<usize, CatalogError> {
    if !scanned.root_readable || scanned.candidates.is_empty() {
        return Ok(0);
    }
    let present: HashSet<String> = scanned
        .candidates
        .iter()
        .map(|c| c.path.to_string_lossy().into_owned())
        .collect();
    let tx = catalog.write_tx()?;
    let gone: Vec<i64> = {
        let mut st =
            tx.prepare("SELECT id, path FROM photos WHERE folder_id = ?1 AND missing = 0")?;
        let rows = st.query_map([folder_id], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut gone = Vec::new();
        for row in rows {
            let (id, path) = row?;
            if !present.contains(&path) {
                gone.push(id);
            }
        }
        gone
    };
    for id in &gone {
        tx.execute("UPDATE photos SET missing = 1 WHERE id = ?1", [id])?;
    }
    tx.commit()?;
    Ok(gone.len())
}

/// How many photos catalogued under `folder_id` are not flagged missing.
pub fn present_count(catalog: &Catalog, folder_id: i64) -> Result<usize, CatalogError> {
    let n: i64 = catalog.conn.query_row(
        "SELECT COUNT(*) FROM photos WHERE folder_id = ?1 AND missing = 0",
        [folder_id],
        |r| r.get(0),
    )?;
    Ok(n as usize)
}

/// Flags every photo catalogued under `folder_id` as missing. For the case [`mark_missing`]
/// refuses to decide alone: the folder was read and holds none of its photos any more, and
/// the user confirmed that they are gone (not an unplugged drive's empty mount point).
/// Returns how many rows this call newly flagged.
pub fn mark_missing_confirmed(catalog: &Catalog, folder_id: i64) -> Result<usize, CatalogError> {
    let tx = catalog.write_tx()?;
    let n = tx.execute(
        "UPDATE photos SET missing = 1 WHERE folder_id = ?1 AND missing = 0",
        [folder_id],
    )?;
    tx.commit()?;
    Ok(n)
}
