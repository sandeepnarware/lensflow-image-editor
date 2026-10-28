//! Print, on the `Long` lane: each photo is rendered to a JPEG sized for its cell
//! ([`lf_catalog::print::PrintSpec::render_edge`]) through the disk export, read back and
//! removed, then the pages are written as one PDF ([`lf_catalog::print::pdf`]) through a
//! temporary name. The library opens it for printing, or leaves it where it was saved.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lf_catalog::Catalog;
use lf_catalog::export::{self, Destination, ExportSpec, Format, Metadata, Pattern, Size};
use lf_catalog::print::{Caption, Placed, PrintSpec, pdf};
use lf_pipeline::Gpu;

/// What to print and where the PDF goes.
#[derive(Debug, Clone, PartialEq)]
pub struct PrintRun {
    pub spec: PrintSpec,
    pub out: PathBuf,
    /// Open it in the system's viewer to print, once written.
    pub open: bool,
}

/// Renders `ids` and writes the PDF; returns its path. `progress(done, total)` after each
/// photo; `stop` ends the run before the next one (and nothing is written).
pub fn run(
    gpu: Arc<Gpu>,
    root: &Path,
    ids: &[i64],
    job: &PrintRun,
    temp: &Path,
    mut progress: impl FnMut(usize, usize),
    stop: &AtomicBool,
) -> Result<PathBuf, String> {
    let catalog = Catalog::open(root).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(temp)
        .map_err(|e| format!("The print folder could not be made ({e})."))?;
    let spec = ExportSpec {
        format: Format::Jpeg { quality: 92 },
        size: Size::LongEdge(job.spec.render_edge()),
        destination: Destination::Folder(temp.to_path_buf()),
        rename: Pattern("{seq}".into()),
        metadata: Metadata::None,
        overwrite: true,
        sharpen: lf_catalog::finish::Sharpen::Standard,
        watermark: None,
    };
    let mut placed = Vec::with_capacity(ids.len());
    progress(0, ids.len());
    for (i, &id) in ids.iter().enumerate() {
        if stop.load(Ordering::SeqCst) {
            return Err("Printing was stopped.".to_owned());
        }
        let row = catalog.photo(id).map_err(|e| e.to_string())?;
        let report = export::run(gpu.clone(), &catalog, &[id], &spec, &mut |_, _| {
            std::ops::ControlFlow::Continue(())
        })
        .map_err(|e| e.to_string())?;
        if report.device_lost {
            return Err("The graphics device was reset.".to_owned());
        }
        let Some(done) = report.exported.first() else {
            let why = report.failed.first().map_or_else(
                || "It could not be rendered.".to_owned(),
                |(_, e)| e.to_string(),
            );
            return Err(why);
        };
        let jpeg = std::fs::read(&done.path)
            .map_err(|e| format!("The rendered photo could not be read ({e})."))?;
        let _ = std::fs::remove_file(&done.path);
        let file_name = row
            .as_ref()
            .map(|r| crate::develop::open::file_name(&r.path))
            .unwrap_or_default();
        let caption = match job.spec.caption {
            Caption::None => None,
            Caption::FileName => Some(file_name),
            Caption::Title => Some(
                row.as_ref()
                    .and_then(|r| r.meta.title.clone())
                    .unwrap_or(file_name),
            ),
        };
        placed.push(Placed {
            jpeg,
            width: done.width,
            height: done.height,
            caption,
        });
        progress(i + 1, ids.len());
    }
    let doc = pdf(&job.spec, &placed);
    let tmp = job.out.with_extension("pdf.tmp");
    std::fs::write(&tmp, doc)
        .and_then(|()| std::fs::rename(&tmp, &job.out))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("The PDF could not be written ({e}).")
        })?;
    Ok(job.out.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lf_catalog::print::{Orientation, PAPERS};

    #[test]
    fn photos_become_a_pdf_with_their_captions() {
        let Some(gpu) = lf_pipeline::test_support::try_shared_gpu() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        for i in 0..3u8 {
            image::RgbaImage::from_pixel(60, 40, image::Rgba([60 * i, 90, 160, 255]))
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
        catalog
            .set_meta(&ids[..1], lf_catalog::meta::Field::Title, "First dance")
            .unwrap();
        let job = PrintRun {
            spec: PrintSpec {
                paper: PAPERS[0],
                orientation: Orientation::Auto,
                cols: 2,
                rows: 2,
                margin_mm: 10.0,
                gap_mm: 5.0,
                caption: Caption::Title,
            },
            out: dir.path().join("sheet.pdf"),
            open: false,
        };
        let mut steps = Vec::new();
        let out = run(
            gpu,
            dir.path(),
            &ids,
            &job,
            &dir.path().join("print-temp"),
            |d, t| steps.push((d, t)),
            &AtomicBool::new(false),
        )
        .unwrap();
        let doc = std::fs::read(&out).unwrap();
        let text = String::from_utf8_lossy(&doc);
        assert!(text.starts_with("%PDF"));
        assert_eq!(text.matches("/Subtype /Image").count(), 3);
        assert!(text.contains("(First dance) Tj"));
        assert!(text.contains("(p1.png) Tj"), "no title: the file name");
        assert_eq!(steps.last(), Some(&(3, 3)));
        assert_eq!(
            std::fs::read_dir(dir.path().join("print-temp"))
                .unwrap()
                .count(),
            0,
            "no rendered file is left behind"
        );
    }
}
