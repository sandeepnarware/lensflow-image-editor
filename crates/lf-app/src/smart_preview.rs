//! Smart previews (Lightroom's): a smaller copy of each photo kept in the catalogue, so a photo
//! whose drive is not connected can still be opened and edited in Develop. The edits go to the
//! catalogue as always; the photo's sidecar is written the next time the original is opened
//! (the catalogue is newer then).
//!
//! A preview is the photo developed as the catalogue says (its RAW settings) at [`EDGE`] pixels,
//! kept in `<catalogue>/.lensflow/previews/` in the frame cache's own format
//! ([`lf_raw::cache::FrameCache`]), named by the photo's id
//! ([`lf_raw::cache::CacheKey::named`]). Making one again replaces it.

use lf_catalog::{Catalog, edits};
use lf_raw::cache::{CacheKey, FrameCache};
use lf_raw::demosaic::Quality;
use lf_raw::develop::Developed;
use lf_raw::open::{Kind, Opened, Picture};
use lf_settings::{DevelopSettings, EditSettings};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// The most a preview's long edge is, in pixels (the open shrinks by whole factors, so it may
/// be as little as half).
pub const EDGE: u32 = 2560;
pub const NOTE: &str = "Smart preview: the original is offline. Edits are kept in the catalogue.";

pub fn dir(root: &Path) -> PathBuf {
    root.join(".lensflow").join("previews")
}

fn key(id: i64) -> CacheKey {
    CacheKey::named(&format!("photo-{id}"))
}

/// Photo `id` has a smart preview.
pub fn has(root: &Path, id: i64) -> bool {
    dir(root).join(key(id).file_name()).is_file()
}

fn store(root: &Path) -> Result<FrameCache, String> {
    FrameCache::open(&dir(root), u64::MAX).map_err(|e| e.to_string())
}

/// Makes the previews of `photos` (id, file); returns how many were made, and how many could
/// not be read. `stop` ends it between photos.
pub fn build(
    root: &Path,
    photos: &[(i64, PathBuf)],
    mut progress: impl FnMut(usize, usize),
    stop: &AtomicBool,
) -> Result<(usize, usize), String> {
    let cache = store(root)?;
    let catalog = Catalog::open(root).map_err(|e| e.to_string())?;
    let (mut made, mut failed) = (0, 0);
    for (i, (id, path)) in photos.iter().enumerate() {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        progress(i, photos.len());
        let settings = edits::get_settings(&catalog, *id)
            .map_err(|e| e.to_string())?
            .unwrap_or_else(EditSettings::default_settings);
        let develop = DevelopSettings::from_settings(&settings);
        match lf_raw::open::open(path, Quality::Best, &develop, EDGE) {
            Ok(opened) => {
                let developed = match opened.picture {
                    Picture::Float(d) => d,
                    Picture::EightBit(img) => {
                        Developed::from_rgba8(&img, (opened.full_width, opened.full_height))
                    }
                };
                cache.put(&key(*id), &developed);
                made += 1;
            }
            Err(e) => {
                log::warn!("no smart preview for {}: {e}", path.display());
                failed += 1;
            }
        }
    }
    progress(photos.len(), photos.len());
    Ok((made, failed))
}

/// Photo `id` (whose file is `path`) opened from its smart preview: a RAW's kind kept, so its
/// panels show, though its develop settings are those the preview was made with.
pub fn open(root: &Path, id: i64, path: &Path) -> Result<Opened, String> {
    let developed = store(root)?
        .get(&key(id))
        .ok_or_else(|| "The photo is offline and has no smart preview.".to_owned())?;
    let raw = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .is_some_and(|e| lf_catalog::stacks::RAW_EXTENSIONS.contains(&e.as_str()));
    Ok(Opened {
        full_width: developed.full_width,
        full_height: developed.full_height,
        reduced: true,
        picture: Picture::Float(developed),
        kind: if raw { Kind::Raw } else { Kind::Jpeg },
        exif: Default::default(),
        jpeg_exif_bytes: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_photo_is_opened_from_its_preview_once_its_file_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.png");
        image::RgbaImage::from_pixel(3000, 2000, image::Rgba([200, 100, 50, 255]))
            .save(&path)
            .unwrap();
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
        let id = catalog.photo_ids().unwrap()[0];
        assert!(!has(dir.path(), id));
        let (made, failed) = build(
            dir.path(),
            &[(id, path.clone())],
            |_, _| {},
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!((made, failed), (1, 0));
        assert!(has(dir.path(), id));
        std::fs::remove_file(&path).unwrap();
        let opened = open(dir.path(), id, &path).unwrap();
        assert_eq!((opened.full_width, opened.full_height), (3000, 2000));
        let Picture::Float(d) = &opened.picture else {
            panic!("a frame")
        };
        assert!(
            (EDGE / 2..=EDGE).contains(&d.frame.width),
            "made smaller, by a whole factor: {}",
            d.frame.width
        );
        let red = d.frame.data[0].to_f32();
        assert!((red - 200.0 / 255.0).abs() < 0.01, "{red}");
    }
}
