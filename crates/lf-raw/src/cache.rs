//! A disk cache of developed frames, so reopening a photo skips the demosaic.
//!
//! One file per entry, named by [`CacheKey`] (`<sha256 hex>.lff`):
//!
//! ```text
//! "LFF1" | version u16 (=1) | width u32 | height u32 | full_width u32 | full_height u32
//! | flags u8 (bit 0 reduced, bit 1 orientation applied)
//! | lz4 block of the f16 RGBA bytes (little endian) | crc32 u32 of that block
//! ```
//!
//! All integers are little endian. A truncated, corrupt or version-mismatched entry is
//! treated as a miss and deleted.
//!
//! Last access is tracked in a small `<key>.atime` sidecar holding a strictly increasing
//! counter (nanoseconds since the epoch, bumped to stay unique within the process). File
//! mtimes are not used: they are coarse on some file systems (equal for entries written in
//! the same tick) and some volumes are mounted without access-time updates, so they cannot
//! order reads reliably. A sidecar rewrite is cheap and works the same everywhere.

use crate::RawError;
use crate::demosaic::Quality;
use crate::develop::Developed;
use half::f16;
use lf_pipeline::FloatFrame;
use lf_settings::{DevelopSettings, EditSettings};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const MAGIC: &[u8; 4] = b"LFF1";
const VERSION: u16 = 1;
const HEADER_LEN: usize = 4 + 2 + 4 * 4 + 1;
const FLAG_REDUCED: u8 = 1;
const FLAG_ORIENTED: u8 = 2;
/// Refuse to allocate more than this when decoding (a damaged header may claim anything).
const MAX_RAW_BYTES: u64 = 1 << 32;

/// Identifies one developed frame: the file's content and stat, the develop settings,
/// the demosaic quality and the size limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheKey {
    name: String,
}

impl CacheKey {
    pub fn for_file(
        path: &Path,
        settings: &DevelopSettings,
        quality: Quality,
        max_edge: u32,
    ) -> Result<CacheKey, RawError> {
        let io = |source| RawError::Io {
            path: path.to_path_buf(),
            source,
        };
        let mut file = fs::File::open(path).map_err(io)?;
        let meta = file.metadata().map_err(io)?;
        let mtime = meta
            .modified()
            .map_err(io)?
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or_else(|e| -(e.duration().as_secs() as i64));

        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = file.read(&mut buf).map_err(io)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        hasher.update(meta.len().to_le_bytes());
        hasher.update(mtime.to_le_bytes());
        hasher.update(settings_json(settings).as_bytes());
        hasher.update(
            match quality {
                Quality::Fast => "fast",
                Quality::Best => "best",
            }
            .as_bytes(),
        );
        hasher.update(max_edge.to_le_bytes());
        Ok(CacheKey {
            name: format!("{}.lff", hex::encode(hasher.finalize())),
        })
    }

    /// The entry's file name.
    pub fn file_name(&self) -> &str {
        &self.name
    }
}

fn settings_json(settings: &DevelopSettings) -> String {
    let mut s = EditSettings::default_settings();
    settings.write_into(&mut s);
    s.to_value().to_string()
}

/// A folder of cached developed frames, trimmed to a byte budget.
#[derive(Debug)]
pub struct FrameCache {
    dir: PathBuf,
    cap_bytes: u64,
}

static LAST_STAMP: AtomicU64 = AtomicU64::new(0);

fn next_stamp() -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut prev = LAST_STAMP.load(Ordering::Relaxed);
    loop {
        let next = now.max(prev + 1);
        match LAST_STAMP.compare_exchange_weak(prev, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(p) => prev = p,
        }
    }
}

impl FrameCache {
    pub fn open(dir: &Path, cap_bytes: u64) -> Result<FrameCache, RawError> {
        fs::create_dir_all(dir).map_err(|source| RawError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        Ok(FrameCache {
            dir: dir.to_path_buf(),
            cap_bytes,
        })
    }

    fn entry_path(&self, key: &CacheKey) -> PathBuf {
        self.dir.join(&key.name)
    }

    fn touch(&self, entry: &Path) {
        let _ = fs::write(entry.with_extension("atime"), next_stamp().to_string());
    }

    fn remove(&self, entry: &Path) {
        let _ = fs::remove_file(entry);
        let _ = fs::remove_file(entry.with_extension("atime"));
    }

    fn last_access(&self, entry: &Path, meta: &fs::Metadata) -> u64 {
        fs::read_to_string(entry.with_extension("atime"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or_else(|| {
                meta.modified()
                    .ok()
                    .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(0)
            })
    }

    /// The cached frame, or `None` (a bad entry is deleted). A hit counts as an access.
    pub fn get(&self, key: &CacheKey) -> Option<Developed> {
        let path = self.entry_path(key);
        let bytes = fs::read(&path).ok()?;
        match decode(&bytes) {
            Some(d) => {
                self.touch(&path);
                Some(d)
            }
            None => {
                self.remove(&path);
                None
            }
        }
    }

    /// Store a frame (written to a temp file and renamed, so a reader never sees half of
    /// it), then trim. Failures are logged and otherwise ignored: the cache is optional.
    pub fn put(&self, key: &CacheKey, d: &Developed) {
        static N: AtomicU64 = AtomicU64::new(0);
        let path = self.entry_path(key);
        let tmp = self.dir.join(format!(
            "{}.{}.{}.tmp",
            key.name,
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let result = fs::write(&tmp, encode(d)).and_then(|()| fs::rename(&tmp, &path));
        if let Err(e) = result {
            log::warn!("frame cache: could not write {}: {e}", path.display());
            let _ = fs::remove_file(&tmp);
            return;
        }
        self.touch(&path);
        self.trim();
    }

    /// Delete least recently used entries until the total size is within the cap.
    pub fn trim(&self) {
        let Ok(read) = fs::read_dir(&self.dir) else {
            return;
        };
        let mut entries: Vec<(u64, u64, PathBuf)> = read
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "lff"))
            .filter_map(|e| {
                let meta = e.metadata().ok()?;
                let path = e.path();
                Some((self.last_access(&path, &meta), meta.len(), path))
            })
            .collect();
        let mut total: u64 = entries.iter().map(|e| e.1).sum();
        entries.sort();
        for (_, len, path) in entries {
            if total <= self.cap_bytes {
                break;
            }
            self.remove(&path);
            total -= len;
        }
    }
}

fn encode(d: &Developed) -> Vec<u8> {
    let raw: Vec<u8> = d
        .frame
        .data
        .iter()
        .flat_map(|v| v.to_bits().to_le_bytes())
        .collect();
    let payload = lz4_flex::block::compress(&raw);
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len() + 4);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    for v in [d.frame.width, d.frame.height, d.full_width, d.full_height] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.push(
        if d.reduced { FLAG_REDUCED } else { 0 }
            | if d.orientation_applied {
                FLAG_ORIENTED
            } else {
                0
            },
    );
    out.extend_from_slice(&payload);
    out.extend_from_slice(&crc32fast::hash(&payload).to_le_bytes());
    out
}

fn decode(b: &[u8]) -> Option<Developed> {
    if b.len() < HEADER_LEN + 4 || &b[..4] != MAGIC {
        return None;
    }
    if u16::from_le_bytes([b[4], b[5]]) != VERSION {
        return None;
    }
    let u32_at = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    let (w, h, fw, fh) = (u32_at(6), u32_at(10), u32_at(14), u32_at(18));
    let flags = b[22];
    let raw_len = u64::from(w) * u64::from(h) * 8;
    if raw_len > MAX_RAW_BYTES {
        return None;
    }
    let (payload, crc) = b[HEADER_LEN..].split_at(b.len() - HEADER_LEN - 4);
    if crc32fast::hash(payload).to_le_bytes() != crc {
        return None;
    }
    let raw = lz4_flex::block::decompress(payload, raw_len as usize).ok()?;
    if raw.len() as u64 != raw_len {
        return None;
    }
    let data = raw
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| f16::from_bits(u16::from_le_bytes(*c)))
        .collect();
    Some(Developed {
        frame: FloatFrame {
            width: w,
            height: h,
            data,
        },
        full_width: fw,
        full_height: fh,
        reduced: flags & FLAG_REDUCED != 0,
        orientation_applied: flags & FLAG_ORIENTED != 0,
    })
}
