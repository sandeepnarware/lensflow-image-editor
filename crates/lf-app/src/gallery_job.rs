//! Export to a LensFlow gallery, on the `Long` lane: each photo is rendered to a JPEG through
//! the disk export ([`lf_catalog::export::run`], so it carries its EXIF and the sRGB profile)
//! into a temporary folder, uploaded with [`lf_account::galleries::upload`], and the file
//! removed, so at most one rendered photo is on disk at a time. A photo the gallery already
//! holds counts as a duplicate, not a failure; one that fails is listed and the run goes on.
//!
//! The uploader is passed in ([`Upload`]), so the run is tested without a server; the app
//! gives it [`uploader`], which talks to LensFlow with the studio's client.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lf_account::Client;
use lf_account::galleries::{self, Uploaded};
use lf_catalog::Catalog;
use lf_catalog::export::{self, Destination, ExportSpec, Format, Metadata, Pattern, Size};
use lf_pipeline::Gpu;

/// The studio's client, handed to the library while the gate allows gallery export.
#[derive(Clone)]
pub struct GalleryLink {
    pub client: Client,
}

impl std::fmt::Debug for GalleryLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GalleryLink").finish_non_exhaustive()
    }
}

/// What to upload and where.
#[derive(Debug, Clone, PartialEq)]
pub struct GalleryRun {
    pub gallery: i64,
    pub section: Option<i64>,
    pub size: Size,
    /// JPEG quality, 1..=100.
    pub quality: u8,
}

/// What a finished (or stopped) upload run did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GalleryOutcome {
    pub total: usize,
    pub uploaded: usize,
    /// Already in the gallery (not sent again).
    pub duplicates: usize,
    /// File name and why.
    pub failed: Vec<(String, String)>,
    pub cancelled: bool,
}

/// "12 photos added to the gallery", with the duplicates and failures after.
pub fn done_text(o: &GalleryOutcome) -> String {
    let noun = |n: usize| if n == 1 { "photo" } else { "photos" };
    let mut text = format!(
        "{} {} added to the gallery",
        crate::catalog_ext::count(o.uploaded),
        noun(o.uploaded)
    );
    if o.duplicates > 0 {
        text.push_str(&format!(
            "; {} already there",
            crate::catalog_ext::count(o.duplicates)
        ));
    }
    if !o.failed.is_empty() {
        text.push_str(&format!(
            "; {} could not be added",
            crate::catalog_ext::count(o.failed.len())
        ));
    }
    if o.cancelled {
        text.push_str("; stopped");
    }
    text.push('.');
    text
}

/// Sends one finished JPEG (`name`, its bytes, a progress callback for bytes sent).
pub type Upload<'a> =
    dyn FnMut(&str, &[u8], &mut dyn FnMut(usize)) -> Result<Uploaded, String> + 'a;

/// The real uploader: the studio's client (with the long timeouts) to `run.gallery`.
pub fn uploader(
    link: GalleryLink,
    run: &GalleryRun,
) -> impl FnMut(&str, &[u8], &mut dyn FnMut(usize)) -> Result<Uploaded, String> {
    let client = link.client.long();
    let (gallery, section) = (run.gallery, run.section);
    move |name: &str, bytes: &[u8], sent: &mut dyn FnMut(usize)| {
        galleries::upload(&client, gallery, section, name, bytes, sent).map_err(|e| e.to_string())
    }
}

/// A folder for the rendered JPEGs, inside `cache`, emptied first (a run left behind by a
/// crash leaves no files).
pub fn temp_folder(cache: &Path) -> Result<PathBuf, String> {
    let dir = cache.join("gallery-upload");
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .map_err(|e| format!("The upload folder could not be cleared ({e})."))?;
    }
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("The upload folder could not be made ({e})."))?;
    Ok(dir)
}

/// Renders and uploads `ids` in order. `progress(done, total)` after each photo; `stop`
/// ends the run after the current one. `Err` only when nothing could start (the catalogue
/// could not be opened).
#[allow(clippy::too_many_arguments)]
pub fn run(
    gpu: Arc<Gpu>,
    catalogue_root: &Path,
    ids: &[i64],
    opts: &GalleryRun,
    temp: &Path,
    upload: &mut Upload,
    mut progress: impl FnMut(usize, usize),
    stop: &AtomicBool,
) -> Result<GalleryOutcome, String> {
    let catalog = Catalog::open(catalogue_root).map_err(|e| e.to_string())?;
    let spec = ExportSpec {
        format: Format::Jpeg {
            quality: opts.quality.clamp(1, 100),
        },
        size: opts.size,
        destination: Destination::Folder(temp.to_path_buf()),
        rename: Pattern("{name}".into()),
        metadata: Metadata::All,
        overwrite: true,
        // The gallery makes its own previews and watermarks.
        sharpen: Default::default(),
        watermark: None,
    };
    let mut out = GalleryOutcome {
        total: ids.len(),
        ..GalleryOutcome::default()
    };
    progress(0, ids.len());
    for (i, &id) in ids.iter().enumerate() {
        if stop.load(Ordering::SeqCst) {
            out.cancelled = true;
            break;
        }
        let name_of = |id: i64| {
            catalog
                .photo(id)
                .ok()
                .flatten()
                .map(|p| crate::develop::open::file_name(&p.path))
                .unwrap_or_else(|| format!("photo {id}"))
        };
        let report = export::run(gpu.clone(), &catalog, &[id], &spec, &mut |_, _| {
            std::ops::ControlFlow::Continue(())
        });
        let file = match report {
            Ok(r) if r.device_lost => {
                out.failed
                    .push((name_of(id), "The graphics device was reset.".into()));
                break;
            }
            Ok(mut r) if !r.exported.is_empty() => r.exported.remove(0).path,
            Ok(r) => {
                let why = r.failed.first().map_or_else(
                    || "It could not be rendered.".to_owned(),
                    |(_, e)| e.to_string(),
                );
                out.failed.push((name_of(id), why));
                progress(i + 1, ids.len());
                continue;
            }
            Err(e) => {
                out.failed.push((name_of(id), e.to_string()));
                progress(i + 1, ids.len());
                continue;
            }
        };
        let name = file.file_name().map_or_else(
            || "photo.jpg".to_owned(),
            |n| n.to_string_lossy().into_owned(),
        );
        let sent = std::fs::read(&file)
            .map_err(|e| format!("The rendered photo could not be read ({e})."))
            .and_then(|bytes| upload(&name, &bytes, &mut |_| {}));
        let _ = std::fs::remove_file(&file);
        match sent {
            Ok(Uploaded::Photo(_)) => out.uploaded += 1,
            Ok(Uploaded::Duplicate(_)) => out.duplicates += 1,
            Err(e) => {
                log::warn!("uploading {name} failed: {e}");
                out.failed.push((name, e));
            }
        }
        progress(i + 1, ids.len());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_closing_line_says_what_happened() {
        let o = GalleryOutcome {
            total: 5,
            uploaded: 3,
            duplicates: 1,
            failed: vec![("a.jpg".into(), "no".into())],
            cancelled: false,
        };
        assert_eq!(
            done_text(&o),
            "3 photos added to the gallery; 1 already there; 1 could not be added."
        );
        let one = GalleryOutcome {
            total: 1,
            uploaded: 1,
            ..GalleryOutcome::default()
        };
        assert_eq!(done_text(&one), "1 photo added to the gallery.");
    }

    #[test]
    fn a_run_renders_uploads_and_cleans_up_each_photo() {
        let Some(gpu) = lf_pipeline::test_support::try_shared_gpu() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        for i in 0..3u8 {
            image::RgbaImage::from_pixel(60, 40, image::Rgba([40 * i, 90, 160, 255]))
                .save(dir.path().join(format!("p{i}.png")))
                .unwrap();
        }
        let catalog = Catalog::open(dir.path()).unwrap();
        let folder = catalog.add_folder(&dir.path().to_string_lossy()).unwrap();
        lf_catalog::import::run(
            &catalog,
            folder,
            &lf_catalog::import::scan(dir.path()).candidates,
            200,
            &mut |_| Ok(()),
        )
        .unwrap();
        let ids = catalog.photo_ids().unwrap();
        let temp = temp_folder(&dir.path().join("cache")).unwrap();
        let mut seen = Vec::new();
        // The second is "already there", the third fails; the first is added.
        let mut fake = |name: &str, bytes: &[u8], _: &mut dyn FnMut(usize)| {
            assert!(bytes.starts_with(&[0xff, 0xd8]), "a JPEG");
            seen.push(name.to_owned());
            match seen.len() {
                1 => Ok(Uploaded::Photo(10)),
                2 => Ok(Uploaded::Duplicate(Some(4))),
                _ => Err("The gallery is archived.".to_owned()),
            }
        };
        let mut steps = Vec::new();
        let out = run(
            gpu,
            dir.path(),
            &ids,
            &GalleryRun {
                gallery: 5,
                section: None,
                size: Size::LongEdge(30),
                quality: 80,
            },
            &temp,
            &mut fake,
            |d, t| steps.push((d, t)),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(seen, ["p0.jpg", "p1.jpg", "p2.jpg"]);
        assert_eq!((out.uploaded, out.duplicates), (1, 1));
        assert_eq!(
            out.failed,
            [("p2.jpg".to_owned(), "The gallery is archived.".to_owned())]
        );
        assert_eq!(steps.last(), Some(&(3, 3)));
        assert_eq!(
            std::fs::read_dir(&temp).unwrap().count(),
            0,
            "no rendered file is left behind"
        );
    }

    #[test]
    fn stop_ends_the_run_before_the_next_photo() {
        let Some(gpu) = lf_pipeline::test_support::try_shared_gpu() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let catalog = Catalog::open(dir.path()).unwrap();
        drop(catalog);
        let stop = AtomicBool::new(true);
        let mut never = |_: &str, _: &[u8], _: &mut dyn FnMut(usize)| -> Result<Uploaded, String> {
            panic!("nothing is sent once stopped")
        };
        let out = run(
            gpu,
            dir.path(),
            &[1, 2],
            &GalleryRun {
                gallery: 5,
                section: None,
                size: Size::Original,
                quality: 90,
            },
            &dir.path().join("t"),
            &mut never,
            |_, _| {},
            &stop,
        )
        .unwrap();
        assert!(out.cancelled);
        assert_eq!(out.uploaded, 0);
    }
}
