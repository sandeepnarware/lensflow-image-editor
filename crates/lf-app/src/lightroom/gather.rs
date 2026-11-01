//! Gathering preset files into memory for the import job.
//!
//! The picker hands over loose `.xmp` / `.lrtemplate` files, folders, `.zip` packs, or the
//! folders Adobe's own apps keep presets in. Everything is read into memory and nothing is
//! ever extracted to disk: a pack is untrusted data, so its entry count, declared sizes and
//! the bytes actually read are all capped (declared sizes can lie).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Largest preset file taken (a real preset is a few KB; this is generous).
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// Largest total a zip pack may unpack to, summed over its entries' declared sizes.
pub const MAX_ZIP_BYTES: u64 = 50 * 1024 * 1024;
/// Most entries a zip pack may hold.
pub const MAX_ZIP_ENTRIES: usize = 5_000;

/// Shown when a file is bigger than [`MAX_FILE_BYTES`].
pub const FILE_TOO_BIG: &str = "This file is too large to be a preset, so it was skipped.";
/// Shown when a pack would unpack to more than [`MAX_ZIP_BYTES`].
pub const ZIP_TOO_BIG: &str = "This pack unpacks to more than 50 MB, so it was not opened.";
/// Shown when a pack holds more than [`MAX_ZIP_ENTRIES`] entries.
pub const ZIP_TOO_MANY: &str = "This pack holds more than 5,000 files, so it was not opened.";
/// Shown when a pack cannot be read as a zip file.
pub const ZIP_UNREADABLE: &str = "This pack could not be opened. It may be damaged.";
/// Shown when a file inside a pack cannot be read.
pub const ENTRY_UNREADABLE: &str = "This file inside the pack could not be read.";
/// Shown when a file cannot be read.
pub const FILE_UNREADABLE: &str = "This file could not be read.";
/// Shown when a folder cannot be listed.
pub const FOLDER_UNREADABLE: &str = "This folder could not be read.";

/// One preset file read into memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// Where it came from, as shown to the user: `pack.zip / Moody.xmp`.
    pub source: String,
    /// The name of the folder it sat in, offered as the preset group.
    pub group_hint: Option<String>,
    pub bytes: Vec<u8>,
    /// The bare file name, with extension.
    pub file_name: String,
}

/// What was gathered, and what could not be (`(source, message)` with a ready-to-show sentence).
#[derive(Debug, Default)]
pub struct Gathered {
    pub found: Vec<Found>,
    pub failed: Vec<(String, String)>,
}

/// Is this a preset file by its name?
fn is_preset(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".xmp") || lower.ends_with(".lrtemplate")
}

fn is_zip(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".zip")
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn parent_name(path: &Path) -> Option<String> {
    path.parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
}

/// Reads every preset reachable from `paths`: files, folders (recursively) and zip packs.
/// Other files are ignored. `stop` is checked between files.
pub fn gather(paths: &[PathBuf], stop: &AtomicBool) -> Gathered {
    let mut out = Gathered::default();
    for path in paths {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        gather_path(path, stop, &mut out);
    }
    out
}

fn gather_path(path: &Path, stop: &AtomicBool, out: &mut Gathered) {
    let name = file_name_of(path);
    let shown = path.display().to_string();
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_dir() => gather_dir(path, stop, out),
        Ok(_) if is_zip(&name) => gather_zip(path, stop, out),
        Ok(meta) if is_preset(&name) => {
            if meta.len() > MAX_FILE_BYTES {
                out.failed.push((shown, FILE_TOO_BIG.into()));
                return;
            }
            read_file(path, shown, out);
        }
        Ok(_) => {}
        Err(_) => out.failed.push((shown, FILE_UNREADABLE.into())),
    }
}

fn read_file(path: &Path, source: String, out: &mut Gathered) {
    let read = std::fs::File::open(path).and_then(|f| {
        let mut bytes = Vec::new();
        // One byte past the cap: a file that grew since `metadata` is still refused.
        f.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
        Ok(bytes)
    });
    match read {
        Ok(bytes) if bytes.len() as u64 > MAX_FILE_BYTES => {
            out.failed.push((source, FILE_TOO_BIG.into()));
        }
        Ok(bytes) => out.found.push(Found {
            source,
            group_hint: parent_name(path),
            bytes,
            file_name: file_name_of(path),
        }),
        Err(_) => out.failed.push((source, FILE_UNREADABLE.into())),
    }
}

fn gather_dir(dir: &Path, stop: &AtomicBool, out: &mut Gathered) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        out.failed
            .push((dir.display().to_string(), FOLDER_UNREADABLE.into()));
        return;
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        // `file_type` does not follow links, so a link loop cannot trap the walk.
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            gather_dir(&path, stop, out);
        } else if kind.is_file() {
            let name = file_name_of(&path);
            if is_zip(&name) {
                gather_zip(&path, stop, out);
            } else if is_preset(&name) {
                gather_path(&path, stop, out);
            }
        }
    }
}

fn gather_zip(path: &Path, stop: &AtomicBool, out: &mut Gathered) {
    let pack = file_name_of(path);
    let shown = path.display().to_string();
    let Ok(file) = std::fs::File::open(path) else {
        out.failed.push((shown, FILE_UNREADABLE.into()));
        return;
    };
    let Ok(mut archive) = zip::ZipArchive::new(file) else {
        out.failed.push((shown, ZIP_UNREADABLE.into()));
        return;
    };
    if archive.len() > MAX_ZIP_ENTRIES {
        out.failed.push((shown, ZIP_TOO_MANY.into()));
        return;
    }
    // Declared sizes first: a pack that claims to unpack past the cap is refused whole.
    let mut declared = 0u64;
    for i in 0..archive.len() {
        match archive.by_index_raw(i) {
            Ok(entry) => declared = declared.saturating_add(entry.size()),
            Err(_) => {
                out.failed.push((shown, ZIP_UNREADABLE.into()));
                return;
            }
        }
    }
    if declared > MAX_ZIP_BYTES {
        out.failed.push((shown, ZIP_TOO_BIG.into()));
        return;
    }

    let mut taken = 0u64;
    for i in 0..archive.len() {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let Ok(entry) = archive.by_index(i) else {
            out.failed
                .push((format!("{pack} / #{}", i + 1), ENTRY_UNREADABLE.into()));
            continue;
        };
        if entry.is_dir() {
            continue;
        }
        // A normalised name that cannot escape (absolute and `..` names give None).
        let Some(inner) = entry.enclosed_name() else {
            continue;
        };
        let entry_name = file_name_of(&inner);
        // macOS resource forks and metadata are not presets.
        if !is_preset(&entry_name)
            || entry_name.starts_with("._")
            || inner.components().any(|c| c.as_os_str() == "__MACOSX")
        {
            continue;
        }
        let source = format!("{pack} / {}", inner.to_string_lossy().replace('\\', "/"));
        if entry.size() > MAX_FILE_BYTES {
            out.failed.push((source, FILE_TOO_BIG.into()));
            continue;
        }
        let mut bytes = Vec::new();
        // The declared size can lie: stop at the cap and refuse anything past it.
        if entry
            .take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .is_err()
        {
            out.failed.push((source, ENTRY_UNREADABLE.into()));
            continue;
        }
        taken += bytes.len() as u64;
        if bytes.len() as u64 > MAX_FILE_BYTES || taken > MAX_ZIP_BYTES {
            out.failed.push((source, FILE_TOO_BIG.into()));
            continue;
        }
        out.found.push(Found {
            source,
            group_hint: parent_name(&inner),
            bytes,
            file_name: entry_name,
        });
    }
}

/// Adobe's own preset folders that exist on this machine: Camera Raw's settings and
/// Lightroom's develop presets. Empty on Linux, where Adobe ships no such apps.
pub fn adobe_folders() -> Vec<PathBuf> {
    adobe_candidates()
        .into_iter()
        .filter(|p| p.is_dir())
        .collect()
}

#[cfg(windows)]
fn adobe_candidates() -> Vec<PathBuf> {
    let Some(appdata) = std::env::var_os("APPDATA") else {
        return Vec::new();
    };
    let adobe = PathBuf::from(appdata).join("Adobe");
    vec![
        adobe.join("CameraRaw").join("Settings"),
        adobe.join("Lightroom").join("Develop Presets"),
    ]
}

#[cfg(target_os = "macos")]
fn adobe_candidates() -> Vec<PathBuf> {
    let Some(home) = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()) else {
        return Vec::new();
    };
    let adobe = home.join("Library/Application Support/Adobe");
    vec![
        adobe.join("CameraRaw/Settings"),
        adobe.join("Lightroom/Develop Presets"),
    ]
}

#[cfg(not(any(windows, target_os = "macos")))]
fn adobe_candidates() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn no_stop() -> AtomicBool {
        AtomicBool::new(false)
    }

    fn write_zip(path: &Path, entries: &[(&str, Vec<u8>)]) {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, bytes) in entries {
            zip.start_file(*name, options).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn folders_are_read_recursively_with_group_hints() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Moody");
        std::fs::create_dir_all(root.join("Film")).unwrap();
        std::fs::write(root.join("A.xmp"), b"a").unwrap();
        std::fs::write(root.join("Film").join("B.LRTEMPLATE"), b"b").unwrap();
        let got = gather(&[root], &no_stop());
        assert!(got.failed.is_empty());
        assert_eq!(got.found.len(), 2);
        let a = got.found.iter().find(|f| f.file_name == "A.xmp").unwrap();
        assert_eq!(a.group_hint.as_deref(), Some("Moody"));
        assert_eq!(a.bytes, b"a");
        let b = got
            .found
            .iter()
            .find(|f| f.file_name == "B.LRTEMPLATE")
            .unwrap();
        assert_eq!(b.group_hint.as_deref(), Some("Film"));
    }

    #[test]
    fn zip_packs_are_read_in_memory() {
        let dir = tempfile::tempdir().unwrap();
        let pack = dir.path().join("pack.zip");
        write_zip(
            &pack,
            &[
                ("Moody.xmp", b"top".to_vec()),
                ("Film/Grain.lrtemplate", b"nested".to_vec()),
                ("readme.txt", b"ignored".to_vec()),
                ("__MACOSX/._Moody.xmp", b"junk".to_vec()),
            ],
        );
        let got = gather(&[pack], &no_stop());
        assert!(got.failed.is_empty(), "{:?}", got.failed);
        assert_eq!(got.found.len(), 2);
        let top = got
            .found
            .iter()
            .find(|f| f.file_name == "Moody.xmp")
            .unwrap();
        assert_eq!(top.source, "pack.zip / Moody.xmp");
        assert_eq!(top.bytes, b"top");
        let nested = got
            .found
            .iter()
            .find(|f| f.file_name == "Grain.lrtemplate")
            .unwrap();
        assert_eq!(nested.source, "pack.zip / Film/Grain.lrtemplate");
        assert_eq!(nested.group_hint.as_deref(), Some("Film"));
        // Nothing was extracted beside the pack.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_zip_bomb_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let pack = dir.path().join("bomb.zip");
        write_zip(&pack, &[("Big.xmp", vec![b' '; 51 * 1024 * 1024])]);
        let got = gather(&[pack], &no_stop());
        assert!(got.found.is_empty());
        assert_eq!(got.failed.len(), 1);
        assert_eq!(got.failed[0].1, ZIP_TOO_BIG);
    }

    #[test]
    fn an_oversized_file_is_skipped_with_a_line() {
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("Big.xmp");
        std::fs::write(&big, vec![b' '; MAX_FILE_BYTES as usize + 1]).unwrap();
        let got = gather(&[big], &no_stop());
        assert!(got.found.is_empty());
        assert_eq!(got.failed.len(), 1);
        assert_eq!(got.failed[0].1, FILE_TOO_BIG);
    }

    #[test]
    fn other_files_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("photo.jpg"), b"x").unwrap();
        std::fs::write(dir.path().join("notes.txt"), b"x").unwrap();
        let got = gather(
            &[dir.path().to_path_buf(), dir.path().join("photo.jpg")],
            &no_stop(),
        );
        assert!(got.found.is_empty());
        assert!(got.failed.is_empty());
    }

    #[test]
    fn stop_ends_the_gathering() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("A.xmp"), b"a").unwrap();
        let got = gather(&[dir.path().to_path_buf()], &AtomicBool::new(true));
        assert!(got.found.is_empty());
    }
}
