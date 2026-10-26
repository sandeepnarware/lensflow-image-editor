//! Export to disk, on the `Io` lane: the worker side of the export dialog.
//!
//! The job opens its own `Catalog` and calls [`lf_catalog::export::run`]. It holds no account
//! client and no session, so a token revoked while it runs (Review Focus 2) cannot stop it:
//! the user is sent to sign in and the export carries on. Disk export is never gated by the
//! entitlement (only gallery export is). The progress callback stops the run after the
//! current photo when the dialog's Stop flag is set or the graphics device is lost.

use crate::catalog_ext::count;
use lf_catalog::export::{self, ExportSpec, Exported};
use lf_catalog::{Catalog, CatalogError};
use lf_pipeline::Gpu;
use std::ops::ControlFlow;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The typed-path and probe problem: the folder exists but no file can be made in it.
pub const NOT_WRITABLE: &str = "lensflow-ImageEditor cannot save files in that folder.";

/// A photo that could not be exported: its file name and the error as a sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failed {
    pub photo_id: i64,
    pub file: String,
    pub error: String,
}

/// What a finished (or stopped) export did.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportOutcome {
    pub total: usize,
    pub exported: Vec<Exported>,
    pub failed: Vec<Failed>,
    /// Stopped (the Stop button, or the device was lost) before every photo was tried.
    pub cancelled: bool,
}

/// Whether files can be made in `folder`: it is a readable folder and a probe file can be
/// created and removed there. The error is the sentence to show. Touches the disk.
pub fn check_writable(folder: &Path) -> Result<(), String> {
    crate::catalog_ext::check_folder(folder).map_err(str::to_owned)?;
    let probe = folder.join(format!(".lensflow-export-probe-{}.tmp", std::process::id()));
    let made = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .and_then(|mut f| std::io::Write::write_all(&mut f, b"probe"));
    // Removed whether or not the write worked (a stale probe from a crash is replaced too).
    let _ = std::fs::remove_file(&probe);
    made.map_err(|e| {
        log::info!("{} is not writable: {e}", folder.display());
        NOT_WRITABLE.to_owned()
    })
}

/// Exports `ids` from the catalogue at `catalogue_root` as `spec` says. Takes no account
/// `Client` and no session: see the module comment. `progress(done, total)` is called before
/// the first photo and after each one; once `stop` is set (or `gpu.is_lost()`), the run stops
/// after the current photo. Errors are the sentences to show: an unwritable folder, a run
/// refused before anything is written (two photos on one name, an existing file), a lost
/// device, or the catalogue's own message.
pub fn run(
    gpu: Arc<Gpu>,
    catalogue_root: &Path,
    ids: &[i64],
    spec: &ExportSpec,
    mut progress: impl FnMut(usize, usize),
    stop: &AtomicBool,
) -> Result<ExportOutcome, String> {
    let export::Destination::Folder(folder) = &spec.destination;
    check_writable(folder)?;
    let catalog = Catalog::open(catalogue_root).map_err(|e| e.to_string())?;
    let watch = gpu.clone();
    let report = export::run(gpu, &catalog, ids, spec, &mut |done, total| {
        progress(done, total);
        if stop.load(Ordering::SeqCst) || watch.is_lost() {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    })
    .map_err(|e| error_text(&catalog, &e))?;
    let failed = report
        .failed
        .iter()
        .map(|(id, e)| {
            let path = source_name(&catalog, *id);
            let file = path
                .as_deref()
                .map_or_else(|| format!("photo {id}"), |p| file_name(Path::new(p)));
            Failed {
                photo_id: *id,
                file,
                error: failure_sentence(e, path.as_deref()),
            }
        })
        .collect();
    Ok(ExportOutcome {
        total: ids.len(),
        exported: report.exported,
        failed,
        cancelled: report.cancelled,
    })
}

fn file_name(path: &Path) -> String {
    crate::develop::open::file_name(path)
}

/// A photo's source file as the messages name it.
fn source_name(catalog: &Catalog, id: i64) -> Option<String> {
    catalog
        .photo(id)
        .ok()
        .flatten()
        .map(|p| p.path.to_string_lossy().into_owned())
}

/// The message for a run refused up front, or any other run error, as the dialog shows it.
pub fn error_text(catalog: &Catalog, e: &CatalogError) -> String {
    match e {
        CatalogError::NameCollision {
            path,
            photos: (a, b),
        } => {
            let a = source_name(catalog, *a).unwrap_or_else(|| format!("photo {a}"));
            let b = source_name(catalog, *b).unwrap_or_else(|| format!("photo {b}"));
            let (na, nb) = (file_name(Path::new(&a)), file_name(Path::new(&b)));
            // The same file name in two folders: only the whole paths tell them apart.
            let (a, b) = if na.eq_ignore_ascii_case(&nb) {
                (a, b)
            } else {
                (na, nb)
            };
            format!(
                "Two photos would get the same name: {} ({a} and {b}). Change the file name \
                 pattern.",
                file_name(path)
            )
        }
        CatalogError::Exists(path) => format!(
            "{} already exists. Turn on 'Replace existing files' or choose another folder.",
            file_name(path)
        ),
        other => other.to_string(),
    }
}

/// A per-photo error as a sentence: the source path the library puts first is dropped (the
/// file name is shown beside it), the first letter is a capital and it ends with a stop.
fn failure_sentence(e: &CatalogError, path: Option<&str>) -> String {
    let text = match (e, path) {
        (CatalogError::Export(s), Some(p)) => {
            let rest = s
                .strip_prefix(p)
                .and_then(|r| r.strip_prefix(": "))
                .unwrap_or(s);
            // The decoder's own message often names the path again: the file name is enough.
            rest.replace(p, &file_name(Path::new(p)))
        }
        (CatalogError::Export(s), None) => s.clone(),
        (other, _) => other.to_string(),
    };
    let text = text.trim();
    let mut chars = text.chars();
    let mut out: String = match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => "The photo could not be exported".to_owned(),
    };
    if !out.ends_with(['.', '!', '?']) {
        out.push('.');
    }
    out
}

/// The dialog's (or the toast's) closing line.
pub fn done_text(outcome: &ExportOutcome) -> String {
    let photos = |n: usize| {
        if n == 1 {
            "1 photo".to_owned()
        } else {
            format!("{} photos", count(n))
        }
    };
    let exported = outcome.exported.len();
    if outcome.cancelled {
        format!("Stopped after {}.", photos(exported + outcome.failed.len()))
    } else if outcome.failed.is_empty() {
        format!("Exported {}.", photos(exported))
    } else {
        format!("Exported {} of {}.", count(exported), photos(outcome.total))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lf_catalog::export::{Destination, Format, Metadata, Pattern, Size};
    use lf_catalog::import;
    use std::path::PathBuf;

    fn png(path: &Path) {
        image::RgbaImage::from_fn(24, 16, |x, y| {
            image::Rgba([(x * 9) as u8, (y * 9) as u8, 90, 255])
        })
        .save(path)
        .unwrap();
    }

    /// A catalogue at `root` holding every photo under each of `folders` (created there).
    fn catalogue(root: &Path, folders: &[(&str, &[&str])]) -> Vec<i64> {
        let c = Catalog::open(root).unwrap();
        for (folder, files) in folders {
            let dir = root.join(folder);
            std::fs::create_dir_all(&dir).unwrap();
            for f in *files {
                let path = dir.join(f);
                if f.ends_with(".jpg") {
                    image::RgbImage::from_pixel(24, 16, image::Rgb([90, 120, 30]))
                        .save(&path)
                        .unwrap();
                } else {
                    png(&path);
                }
            }
            let id = c.add_folder(&dir.to_string_lossy()).unwrap();
            import::run(&c, id, &import::scan(&dir).candidates, 200, &mut |_| Ok(())).unwrap();
        }
        c.photo_ids().unwrap()
    }

    fn spec(out: &Path) -> ExportSpec {
        ExportSpec {
            format: Format::Png,
            size: Size::Original,
            destination: Destination::Folder(out.to_path_buf()),
            rename: Pattern("{name}".into()),
            metadata: Metadata::None,
            overwrite: false,
        }
    }

    fn files_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn collision_message_names_both_files() {
        let Some(gpu) = lf_pipeline::test_support::try_shared_gpu() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let root = tempfile::tempdir().unwrap();
        // IMG_1.png in two folders: the same file name, so the message gives both paths.
        let ids = catalogue(root.path(), &[("a", &["IMG_1.png"]), ("b", &["IMG_1.png"])]);
        let out = tempfile::tempdir().unwrap();
        let stop = AtomicBool::new(false);
        let mut s = spec(out.path());
        s.format = Format::Jpeg { quality: 90 };
        let e = run(gpu.clone(), root.path(), &ids, &s, |_, _| {}, &stop).unwrap_err();
        let a = root.path().join("a").join("IMG_1.png");
        let b = root.path().join("b").join("IMG_1.png");
        assert_eq!(
            e,
            format!(
                "Two photos would get the same name: IMG_1.jpg ({} and {}). Change the file \
                 name pattern.",
                a.display(),
                b.display()
            )
        );
        assert!(files_in(out.path()).is_empty(), "nothing was written");

        // Different file names with one stem: the names alone.
        let root = tempfile::tempdir().unwrap();
        let ids = catalogue(root.path(), &[("c", &["IMG_1.jpg", "IMG_1.png"])]);
        let e = run(gpu.clone(), root.path(), &ids, &s, |_, _| {}, &stop).unwrap_err();
        assert_eq!(
            e,
            "Two photos would get the same name: IMG_1.jpg (IMG_1.jpg and IMG_1.png). Change \
             the file name pattern."
        );
        assert!(files_in(out.path()).is_empty());

        // An existing file, when not replacing.
        std::fs::write(out.path().join("IMG_1.png"), b"old").unwrap();
        let e = run(
            gpu,
            root.path(),
            &ids[..1],
            &spec(out.path()),
            |_, _| {},
            &stop,
        )
        .unwrap_err();
        assert_eq!(
            e,
            "IMG_1.png already exists. Turn on 'Replace existing files' or choose another \
             folder."
        );
    }

    #[test]
    fn partial_failure_lists_failed_files() {
        let Some(gpu) = lf_pipeline::test_support::try_shared_gpu() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let root = tempfile::tempdir().unwrap();
        let ids = catalogue(root.path(), &[("f", &["a.png", "b.png", "c.png"])]);
        // b.png is damaged after the import.
        std::fs::write(root.path().join("f").join("b.png"), b"not a picture").unwrap();
        let out = tempfile::tempdir().unwrap();
        let stop = AtomicBool::new(false);
        let mut seen = Vec::new();
        let outcome = run(
            gpu,
            root.path(),
            &ids,
            &spec(out.path()),
            |done, total| seen.push((done, total)),
            &stop,
        )
        .unwrap();
        assert_eq!(seen, [(0, 3), (1, 3), (2, 3), (3, 3)]);
        assert_eq!(outcome.total, 3);
        assert_eq!(outcome.exported.len(), 2);
        assert!(!outcome.cancelled);
        assert_eq!(outcome.failed.len(), 1);
        let failed = &outcome.failed[0];
        assert_eq!(failed.file, "b.png");
        assert!(
            !failed.error.contains(&root.path().display().to_string()),
            "the sentence drops the path: {}",
            failed.error
        );
        assert!(failed.error.ends_with('.'), "{}", failed.error);
        assert!(
            failed.error.starts_with(|c: char| c.is_uppercase()),
            "{}",
            failed.error
        );
        assert_eq!(done_text(&outcome), "Exported 2 of 3 photos.");
        assert_eq!(files_in(out.path()), ["a.png", "c.png"]);
    }

    #[test]
    fn stop_flag_breaks_after_current() {
        let Some(gpu) = lf_pipeline::test_support::try_shared_gpu() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let root = tempfile::tempdir().unwrap();
        let ids = catalogue(root.path(), &[("f", &["a.png", "b.png", "c.png"])]);
        let out = tempfile::tempdir().unwrap();
        let stop = AtomicBool::new(false);
        let outcome = run(
            gpu,
            root.path(),
            &ids,
            &spec(out.path()),
            |done, _| {
                if done == 1 {
                    // Stop pressed while the first photo was being exported.
                    stop.store(true, Ordering::SeqCst);
                }
            },
            &stop,
        )
        .unwrap();
        assert!(outcome.cancelled);
        assert_eq!(outcome.exported.len(), 1);
        assert_eq!(files_in(out.path()), ["a.png"]);
        assert_eq!(done_text(&outcome), "Stopped after 1 photo.");
    }

    #[test]
    fn done_texts_read_well_and_unwritable_folders_are_refused() {
        let exported = |n: usize| {
            (0..n)
                .map(|i| Exported {
                    photo_id: i as i64,
                    path: PathBuf::from(format!("{i}.jpg")),
                    width: 1,
                    height: 1,
                })
                .collect::<Vec<_>>()
        };
        let all = ExportOutcome {
            total: 1_200,
            exported: exported(1_200),
            failed: Vec::new(),
            cancelled: false,
        };
        assert_eq!(done_text(&all), "Exported 1,200 photos.");
        let one = ExportOutcome {
            total: 1,
            exported: exported(1),
            failed: Vec::new(),
            cancelled: false,
        };
        assert_eq!(done_text(&one), "Exported 1 photo.");
        let stopped = ExportOutcome {
            total: 40,
            exported: exported(12),
            failed: Vec::new(),
            cancelled: true,
        };
        assert_eq!(done_text(&stopped), "Stopped after 12 photos.");

        let dir = tempfile::tempdir().unwrap();
        assert_eq!(check_writable(dir.path()), Ok(()));
        assert!(files_in(dir.path()).is_empty(), "the probe file is removed");
        assert_eq!(
            check_writable(&dir.path().join("nope")),
            Err(crate::catalog_ext::FOLDER_MISSING.to_owned())
        );
    }
}
