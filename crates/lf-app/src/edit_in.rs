//! "Edit in external editor" (`Ctrl+E`, as Lightroom's "Edit in Photoshop"): the photo with
//! its edits is rendered to a 16-bit TIFF beside it (`<name>-Edit.tif`, or `-Edit-2`, … when
//! taken), opened in the studio's editor, and the photo's folder is rescanned so the copy is
//! catalogued next to the original. The original is never touched; the editor saves into the
//! copy, and a later rescan picks its changes up.
//!
//! The editor is chosen once (an application picker) and kept in `prefs.external_editor`;
//! Preferences can forget it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lf_catalog::Catalog;
use lf_catalog::export::{self, Destination, ExportSpec, Format, Metadata, Pattern, Size};
use lf_pipeline::Gpu;

/// The copy's name for `stem` that is free in `dir` (`-Edit`, then `-Edit-2`, …).
pub fn copy_stem(dir: &Path, stem: &str) -> String {
    let mut n = 1;
    loop {
        let candidate = if n == 1 {
            format!("{stem}-Edit")
        } else {
            format!("{stem}-Edit-{n}")
        };
        if !dir.join(format!("{candidate}.tif")).exists() {
            return candidate;
        }
        n += 1;
    }
}

/// Renders photo `id` to a TIFF beside it; returns the copy's path. `Err` is a sentence.
pub fn render_copy(gpu: Arc<Gpu>, root: &Path, id: i64) -> Result<PathBuf, String> {
    let catalog = Catalog::open(root).map_err(|e| e.to_string())?;
    let photo = catalog
        .photo(id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "The photo is no longer in the catalogue.".to_owned())?;
    let dir = photo
        .path
        .parent()
        .ok_or_else(|| "The photo's folder could not be found.".to_owned())?
        .to_path_buf();
    let stem = photo
        .path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "photo".to_owned());
    // A literal pattern: the stem's own braces must not read as tokens.
    let name = copy_stem(&dir, &stem).replace(['{', '}'], "_");
    let spec = ExportSpec {
        format: Format::Tiff16,
        size: Size::Original,
        destination: Destination::Folder(dir),
        rename: Pattern(name),
        metadata: Metadata::All,
        overwrite: false,
        sharpen: Default::default(),
        watermark: None,
    };
    let report = export::run(gpu, &catalog, &[id], &spec, &mut |_, _| {
        std::ops::ControlFlow::Continue(())
    })
    .map_err(|e| e.to_string())?;
    if report.device_lost {
        return Err("The graphics device was reset.".to_owned());
    }
    match (report.exported.first(), report.failed.first()) {
        (Some(done), _) => Ok(done.path.clone()),
        (None, Some((_, e))) => Err(e.to_string()),
        (None, None) => Err("The photo could not be rendered.".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_copy_takes_the_first_free_name() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(copy_stem(dir.path(), "IMG_1"), "IMG_1-Edit");
        std::fs::write(dir.path().join("IMG_1-Edit.tif"), b"x").unwrap();
        std::fs::write(dir.path().join("IMG_1-Edit-2.tif"), b"x").unwrap();
        assert_eq!(copy_stem(dir.path(), "IMG_1"), "IMG_1-Edit-3");
    }

    #[test]
    fn a_copy_is_rendered_beside_the_photo() {
        let Some(gpu) = lf_pipeline::test_support::try_shared_gpu() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        image::RgbaImage::from_pixel(40, 30, image::Rgba([90, 120, 160, 255]))
            .save(dir.path().join("a.png"))
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
        let first = render_copy(gpu.clone(), dir.path(), id).unwrap();
        assert_eq!(first, dir.path().join("a-Edit.tif"));
        assert_eq!(image::image_dimensions(&first).unwrap(), (40, 30));
        let second = render_copy(gpu, dir.path(), id).unwrap();
        assert_eq!(second, dir.path().join("a-Edit-2.tif"));
    }
}
