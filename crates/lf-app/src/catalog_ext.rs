//! Thin wrappers over `lf-catalog` for the library: the thumbnail quality, opening the
//! catalogue with everything the library needs, importing a folder, and telling whether a
//! folder is on a network drive. Everything here runs on a worker lane, never the UI thread.

use lf_catalog::import::{self, Progress, Scan, Summary};
use lf_catalog::thumbs::Thumbnailer;
use lf_catalog::{Catalog, Folder, PhotoRow};
use lf_pipeline::Gpu;
use lf_raw::cache::FrameCache;
use lf_raw::demosaic::Quality;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// How thumbnails demosaic RAW files: binned 2×2, a quarter of the memory of `Fast`.
pub const THUMB_QUALITY: Quality = Quality::Preview;

/// Files recorded per import transaction.
pub const IMPORT_BATCH: usize = 200;

/// Shown before a catalogue root on a network drive is used.
pub const NETWORK_WARNING: &str =
    "Catalogues on network drives can be damaged. Choose a folder on this computer.";

/// A typed folder path that does not exist.
pub const FOLDER_MISSING: &str = "That folder does not exist.";
/// A typed folder path that names a file.
pub const FOLDER_IS_FILE: &str = "That is a file, not a folder.";
/// A typed folder path that cannot be listed.
pub const FOLDER_UNREADABLE: &str = "lensflow-ImageEditor cannot read that folder.";

/// Whether `path` is a folder the app can use: it exists, is a directory and can be listed.
/// The error is the sentence to show. Touches the disk (and a network drive may be slow), so
/// call it off the UI thread.
pub fn check_folder(path: &Path) -> Result<(), &'static str> {
    if path.as_os_str().is_empty() {
        return Err(FOLDER_MISSING);
    }
    match std::fs::metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(FOLDER_MISSING),
        Err(_) => Err(FOLDER_UNREADABLE),
        Ok(meta) if !meta.is_dir() => Err(FOLDER_IS_FILE),
        Ok(_) => std::fs::read_dir(path)
            .map(|_| ())
            .map_err(|_| FOLDER_UNREADABLE),
    }
}

/// What the library needs once a catalogue root is open.
pub struct Opened {
    pub thumbs: Thumbnailer,
    /// The decoded-frame cache Develop reads (Task 4).
    pub frames: FrameCache,
    pub folders: Vec<Folder>,
}

impl std::fmt::Debug for Opened {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Opened")
            .field("folders", &self.folders.len())
            .finish_non_exhaustive()
    }
}

/// A thumbnail pool built again after a device error.
pub struct Pool(pub Thumbnailer);

impl std::fmt::Debug for Pool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Pool")
    }
}

fn text(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Opens (creating and migrating) `root/.lensflow/catalog.sqlite`, starts the thumbnail
/// pool and opens the frame cache at `frames_dir`.
pub fn open(
    gpu: Arc<Gpu>,
    root: &Path,
    frames_dir: &Path,
    frame_cache_bytes: u64,
) -> Result<Opened, String> {
    let catalog = Catalog::open(root).map_err(text)?;
    let folders = catalog.folders().map_err(text)?;
    drop(catalog);
    let thumbs = pool(gpu, root)?;
    let frames = FrameCache::open(frames_dir, frame_cache_bytes).map_err(text)?;
    Ok(Opened {
        thumbs,
        frames,
        folders,
    })
}

/// A fresh thumbnail pool on the catalogue at `root`.
pub fn pool(gpu: Arc<Gpu>, root: &Path) -> Result<Thumbnailer, String> {
    Thumbnailer::new(gpu, root, Thumbnailer::default_workers(), THUMB_QUALITY).map_err(text)
}

pub fn folders(root: &Path) -> Result<Vec<Folder>, String> {
    Catalog::open(root).and_then(|c| c.folders()).map_err(text)
}

pub fn photos_in(root: &Path, folder_id: i64) -> Result<Vec<PhotoRow>, String> {
    Catalog::open(root)
        .and_then(|c| c.photos_in(folder_id))
        .map_err(text)
}

pub fn remove_folder(root: &Path, folder_id: i64) -> Result<(), String> {
    Catalog::open(root)
        .and_then(|c| c.remove_folder(folder_id))
        .map_err(text)
}

/// Sets a flag, stars or a colour label on the photos `ids`; returns how many changed.
pub fn mark(root: &Path, ids: &[i64], mark: lf_catalog::cull::Mark) -> Result<usize, String> {
    Catalog::open(root)
        .and_then(|c| c.mark(ids, mark))
        .map_err(text)
}

/// Saves a metadata change (title, caption, creator, copyright or keywords) on `ids`.
pub fn change_meta(
    root: &Path,
    ids: &[i64],
    change: &lf_catalog::meta::Change,
) -> Result<usize, String> {
    Catalog::open(root)
        .and_then(|c| c.change_meta(ids, change))
        .map_err(text)
}

/// Marks every photo of the folder missing, once the user confirmed the folder was emptied.
pub fn mark_missing_confirmed(root: &Path, folder_id: i64) -> Result<usize, String> {
    Catalog::open(root)
        .map_err(text)
        .and_then(|c| import::mark_missing_confirmed(&c, folder_id).map_err(text))
}

/// A finished import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Imported {
    pub folder_id: i64,
    pub summary: Summary,
    /// Photos newly flagged missing.
    pub missing: usize,
    /// The folder was read and held none of its files while this many of its photos were
    /// still present: the user is asked before they are marked missing.
    pub emptied: Option<usize>,
}

/// Records `scan` (of `folder`) in the catalogue at `root`, `IMPORT_BATCH` files per
/// transaction, then flags the folder's photos that are gone (only when the scan reached the
/// folder). `stop` ends it after the batch in progress (when the app quits).
pub fn import_folder(
    root: &Path,
    folder: &Path,
    scan: &Scan,
    stop: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<Imported, String> {
    let catalog = Catalog::open(root).map_err(text)?;
    let folder_id = catalog
        .add_folder(&folder.to_string_lossy())
        .map_err(text)?;
    let summary = import::run(
        &catalog,
        folder_id,
        &scan.candidates,
        IMPORT_BATCH,
        &mut |p| {
            progress(p);
            if stop.load(Ordering::SeqCst) {
                Err(import::ImportAbort)
            } else {
                Ok(())
            }
        },
    )
    .map_err(text)?;
    let missing = import::mark_missing(&catalog, folder_id, scan).map_err(text)?;
    let emptied = if scan.root_readable && scan.candidates.is_empty() {
        Some(import::present_count(&catalog, folder_id).map_err(text)?).filter(|n| *n > 0)
    } else {
        None
    };
    Ok(Imported {
        folder_id,
        summary,
        missing,
        emptied,
    })
}

/// "1,200": a count with thousands separators.
pub fn count(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Whether `path` is on a network drive: a UNC path or a remote drive on Windows; elsewhere a
/// mount under `/mnt`, `/media`, `/net` or `/Volumes` whose filesystem is a network one.
pub fn is_network_path(path: &Path) -> bool {
    let shown = path.to_string_lossy();
    if shown.starts_with(r"\\") && !shown.starts_with(r"\\?\") || shown.starts_with(r"\\?\UNC\") {
        return true;
    }
    platform_network(path)
}

#[cfg(windows)]
fn platform_network(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Component;
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
    const DRIVE_REMOTE: u32 = 4;
    let Some(Component::Prefix(prefix)) = path.components().next() else {
        return false;
    };
    let mut root: Vec<u16> = prefix.as_os_str().encode_wide().collect();
    root.extend("\\".encode_utf16());
    root.push(0);
    // SAFETY: `root` is a NUL-terminated wide string that outlives the call.
    unsafe { GetDriveTypeW(root.as_ptr()) == DRIVE_REMOTE }
}

#[cfg(target_os = "macos")]
fn platform_network(path: &Path) -> bool {
    std::process::Command::new("/sbin/mount")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .is_some_and(|table| network_mount(path, &table))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_network(path: &Path) -> bool {
    std::fs::read_to_string("/proc/mounts").is_ok_and(|table| network_mount(path, &table))
}

const NETWORK_FILESYSTEMS: &[&str] = &["cifs", "smbfs", "smb3", "nfs", "nfs4", "afpfs", "webdav"];
const MOUNT_PARENTS: &[&str] = &["/mnt", "/media", "/net", "/Volumes"];

/// Reads a mount table (Linux `/proc/mounts`: `source target fstype options …`, or macOS
/// `mount`: `source on target (fstype, options)`) and tells whether `path` lies in a network
/// filesystem mounted under one of the usual mount folders.
pub fn network_mount(path: &Path, table: &str) -> bool {
    let mut best: Option<(PathBuf, String)> = None;
    for line in table.lines() {
        let Some((target, fstype)) = parse_mount(line) else {
            continue;
        };
        let target = PathBuf::from(target);
        if path.starts_with(&target)
            && best
                .as_ref()
                .is_none_or(|(b, _)| target.as_os_str().len() > b.as_os_str().len())
        {
            best = Some((target, fstype));
        }
    }
    best.is_some_and(|(target, fstype)| {
        NETWORK_FILESYSTEMS.contains(&fstype.as_str())
            && MOUNT_PARENTS.iter().any(|p| target.starts_with(p))
    })
}

fn parse_mount(line: &str) -> Option<(String, String)> {
    if let Some((_, rest)) = line.split_once(" on ") {
        // macOS: `//user@server/share on /Volumes/share (smbfs, nodev, …)`
        let (target, kind) = rest.rsplit_once(" (")?;
        let fstype = kind.split([',', ')']).next()?.trim();
        return Some((target.to_owned(), fstype.to_owned()));
    }
    let mut fields = line.split_whitespace();
    let _source = fields.next()?;
    // `/proc/mounts` escapes spaces in paths as `\040`.
    let target = fields.next()?.replace("\\040", " ");
    let fstype = fields.next()?;
    Some((target, fstype.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_groups_thousands() {
        assert_eq!(count(0), "0");
        assert_eq!(count(999), "999");
        assert_eq!(count(1_200), "1,200");
        assert_eq!(count(1_234_567), "1,234,567");
    }

    #[test]
    fn network_mounts_are_recognised() {
        let linux = "/dev/sda1 / ext4 rw 0 0\n\
                     //nas/photos /mnt/nas cifs rw 0 0\n\
                     server:/export /media/me/My\\040Share nfs4 rw 0 0\n\
                     /dev/sdb1 /media/me/card vfat rw 0 0\n\
                     //nas/other /srv/other cifs rw 0 0\n";
        assert!(network_mount(Path::new("/mnt/nas/2026/shoot"), linux));
        assert!(network_mount(Path::new("/media/me/My Share/a"), linux));
        assert!(!network_mount(Path::new("/media/me/card/DCIM"), linux));
        assert!(!network_mount(Path::new("/home/me/Pictures"), linux));
        assert!(
            !network_mount(Path::new("/srv/other/x"), linux),
            "only under the usual mount folders"
        );
        let mac = "/dev/disk3s1 on / (apfs, local, journaled)\n\
                   //me@nas._smb._tcp.local/photos on /Volumes/photos (smbfs, nodev, nosuid, mounted by me)\n\
                   /dev/disk4s1 on /Volumes/Card (msdos, local, nodev)\n";
        assert!(network_mount(Path::new("/Volumes/photos/2026"), mac));
        assert!(!network_mount(Path::new("/Volumes/Card/DCIM"), mac));
        assert!(!network_mount(Path::new("/Users/me/Pictures"), mac));
    }

    #[test]
    fn unc_paths_are_network_paths() {
        assert!(is_network_path(Path::new(r"\\nas\photos\2026")));
        assert!(is_network_path(Path::new(r"\\?\UNC\nas\photos")));
        assert!(!is_network_path(Path::new(r"\\?\C:\Users\me\Pictures")));
    }

    #[test]
    fn check_folder_explains_what_is_wrong() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("photo.jpg");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(check_folder(dir.path()), Ok(()));
        assert_eq!(check_folder(&dir.path().join("nope")), Err(FOLDER_MISSING));
        assert_eq!(check_folder(&file), Err(FOLDER_IS_FILE));
        assert_eq!(check_folder(Path::new("")), Err(FOLDER_MISSING));
        assert_eq!(FOLDER_MISSING, "That folder does not exist.");
        assert_eq!(FOLDER_IS_FILE, "That is a file, not a folder.");
        assert_eq!(
            FOLDER_UNREADABLE,
            "lensflow-ImageEditor cannot read that folder."
        );
    }
}
