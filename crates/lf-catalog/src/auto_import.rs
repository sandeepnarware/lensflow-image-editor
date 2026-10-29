//! Auto import (Lightroom's): a watched folder, where tethering software or a card reader
//! drops new photos, is looked at every few seconds; each photo there whose size and time
//! have stopped changing (the camera has finished writing it) is moved into a catalogued
//! folder, which is then scanned as usual. The watched folder itself is never catalogued, so
//! photos only ever live in the destination.
//!
//! Moving keeps the file's name, or adds `-2`, `-3`… when the destination has one already; a
//! move across drives copies, checks the copy's size and only then removes the original.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// What a file looked like at the last look: its size and modification time.
pub type Seen = HashMap<PathBuf, (u64, Option<SystemTime>)>;

/// The photos in `watch` (not its subfolders) that are ready: seen at the last look with the
/// same size and time, and not empty. `seen` is updated for the next look (files gone are
/// forgotten).
pub fn ready(watch: &Path, seen: &mut Seen) -> io::Result<Vec<PathBuf>> {
    let mut now: Seen = HashMap::new();
    for entry in std::fs::read_dir(watch)? {
        let entry = entry?;
        let path = entry.path();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() || !crate::import::is_photo(&path) {
            continue;
        }
        now.insert(path, (meta.len(), meta.modified().ok()));
    }
    let mut out: Vec<PathBuf> = now
        .iter()
        .filter(|(path, state)| state.0 > 0 && seen.get(*path) == Some(state))
        .map(|(path, _)| path.clone())
        .collect();
    out.sort();
    *seen = now;
    Ok(out)
}

/// The free path in `dir` for `name`: itself, else `stem-2.ext`, `stem-3.ext`…
pub fn free_path(dir: &Path, name: &std::ffi::OsStr) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let p = Path::new(name);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = p
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (2..)
        .map(|n| dir.join(format!("{stem}-{n}{ext}")))
        .find(|c| !c.exists())
        .expect("a free name")
}

/// Moves `file` into `dir` (see the module notes); returns where it went.
pub fn move_into(file: &Path, dir: &Path) -> io::Result<PathBuf> {
    let name = file
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let to = free_path(dir, name);
    if std::fs::rename(file, &to).is_ok() {
        return Ok(to);
    }
    // Another drive: copy, check, then remove the original.
    let size = std::fs::metadata(file)?.len();
    let copied = std::fs::copy(file, &to)?;
    if copied != size || std::fs::metadata(&to)?.len() != size {
        let _ = std::fs::remove_file(&to);
        return Err(io::Error::other("the copy was incomplete"));
    }
    std::fs::remove_file(file)?;
    Ok(to)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_photo_is_ready_once_it_stops_changing_and_moves_under_a_free_name() {
        let watch = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let a = watch.path().join("IMG_1.jpg");
        std::fs::write(&a, b"half").unwrap();
        std::fs::write(watch.path().join("notes.txt"), b"x").unwrap();
        std::fs::write(watch.path().join("empty.jpg"), b"").unwrap();
        let mut seen = Seen::new();
        assert!(
            ready(watch.path(), &mut seen).unwrap().is_empty(),
            "first look"
        );
        // Still being written: bigger at the next look.
        std::fs::write(&a, b"half and the rest").unwrap();
        assert!(ready(watch.path(), &mut seen).unwrap().is_empty());
        assert_eq!(ready(watch.path(), &mut seen).unwrap(), vec![a.clone()]);
        // The destination has an IMG_1.jpg already.
        std::fs::write(dest.path().join("IMG_1.jpg"), b"older").unwrap();
        let to = move_into(&a, dest.path()).unwrap();
        assert_eq!(to, dest.path().join("IMG_1-2.jpg"));
        assert!(!a.exists());
        assert_eq!(std::fs::read(&to).unwrap(), b"half and the rest");
    }
}
