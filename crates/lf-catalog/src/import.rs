//! Folder import: find candidates, then record them in the catalogue in resumable batches.
//! Only headers and EXIF are read; a file whose headers lack a size is decoded small.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use lf_raw::RawError;
use lf_raw::demosaic::Quality;
use lf_raw::open::{Kind, summary};
use lf_settings::DevelopSettings;

use crate::db::{Catalog, CatalogError};
use crate::model::{now, ts};

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

/// Every supported file under `root`, recursively, skipping hidden entries, the `.lensflow`
/// folder and `*.lensflow.json` sidecars. Unreadable directories are skipped.
pub fn scan(root: &Path) -> Vec<Candidate> {
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
    out
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

fn unreadable(e: &RawError) -> Meta {
    Meta {
        kind: "unreadable".into(),
        width: None,
        height: None,
        camera: None,
        lens: None,
        captured_at: None,
        orientation: 1,
        error: Some(e.to_string()),
    }
}

/// Import `candidates` into `folder_id`, `batch` files per transaction. After each batch has
/// committed, and every 25 files within a batch, `progress` is called; an `Err` stops the import,
/// keeping what was committed (an in-progress batch is dropped).
/// Files already catalogued with the same size and mtime are skipped, so a rerun resumes.
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
        let mut existing: HashMap<String, (i64, i64, i64)> = HashMap::new();
        {
            let mut st = catalog
                .conn
                .prepare("SELECT id, file_size, mtime FROM photos WHERE path = ?1")?;
            for c in chunk {
                let key = c.path.to_string_lossy().into_owned();
                let mut rows = st.query([&key])?;
                if let Some(r) = rows.next()? {
                    existing.insert(key, (r.get(0)?, r.get(1)?, r.get(2)?));
                }
            }
        }
        let mut work = Vec::new();
        for (i, c) in chunk.iter().enumerate() {
            let key = c.path.to_string_lossy().into_owned();
            match existing.get(&key) {
                Some(&(_, size, mtime)) if size == c.size as i64 && mtime == c.mtime => {
                    summary.skipped += 1;
                }
                found => {
                    let meta = read_meta(&c.path).unwrap_or_else(|e| unreadable(&e));
                    work.push((key, c, found.map(|f| f.0), meta));
                }
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
        if !work.is_empty() {
            let tx = catalog.write_tx()?;
            let t = ts(now());
            for (key, c, id, m) in &work {
                match id {
                    Some(id) => {
                        tx.execute(
                            "UPDATE photos SET file_size=?2, mtime=?3, width=?4, height=?5, camera=?6,
                             lens=?7, captured_at=?8, kind=?9, orientation=?10, error=?11, updated_at=?12
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
                    }
                    None => {
                        tx.execute(
                            "INSERT INTO photos (folder_id, path, file_size, mtime, width, height,
                             camera, lens, captured_at, kind, orientation, error, imported_at, updated_at)
                             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?13)",
                            rusqlite::params![
                                folder_id,
                                key,
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
                    }
                }
            }
            tx.commit()?;
        }
        done += chunk.len();
        if progress(Progress { done, total }).is_err() {
            return Err(ImportError::Aborted);
        }
    }
    Ok(summary)
}
