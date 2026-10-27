//! The export check by hand: `cargo run -p lf-app --example export_corpus -- <raw file>`
//! copies the RAW into a temporary catalogue, exports it through `export_job::run` as a
//! JPEG (quality 93) and as a 16-bit TIFF, and prints each file's size and dimensions.

use lf_app::export_job;
use lf_catalog::export::{Destination, ExportSpec, Format, Metadata, Pattern, Size};
use lf_catalog::{Catalog, import};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

fn main() {
    let raw = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: export_corpus <raw file>"),
    );
    let gpu = lf_pipeline::Gpu::new().expect("a graphics adapter");
    let work = tempfile::tempdir().unwrap();
    let shoot = work.path().join("shoot");
    std::fs::create_dir(&shoot).unwrap();
    std::fs::copy(&raw, shoot.join(raw.file_name().unwrap())).unwrap();
    let root = work.path().join("catalogue");
    std::fs::create_dir(&root).unwrap();
    let ids = {
        let c = Catalog::open(&root).unwrap();
        let folder = c.add_folder(&shoot.to_string_lossy()).unwrap();
        import::run(
            &c,
            folder,
            &import::scan(&shoot).candidates,
            200,
            &mut |_| Ok(()),
        )
        .unwrap();
        c.photo_ids().unwrap()
    };
    let out = work.path().join("out");
    std::fs::create_dir(&out).unwrap();
    for format in [Format::Jpeg { quality: 93 }, Format::Tiff16] {
        let spec = ExportSpec {
            format,
            size: Size::Original,
            destination: Destination::Folder(out.clone()),
            rename: Pattern("{name}".into()),
            metadata: Metadata::All,
            overwrite: false,
            sharpen: Default::default(),
            watermark: None,
        };
        let started = std::time::Instant::now();
        let outcome = export_job::run(
            gpu.clone(),
            &root,
            &ids,
            &spec,
            |_, _| {},
            &AtomicBool::new(false),
        )
        .expect("the export runs");
        assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
        for e in &outcome.exported {
            let bytes = std::fs::metadata(&e.path).unwrap().len();
            println!(
                "{format:?}: {} {}x{} {bytes} bytes ({:.1} MB) in {:.1} s",
                e.path.file_name().unwrap().to_string_lossy(),
                e.width,
                e.height,
                bytes as f64 / 1e6,
                started.elapsed().as_secs_f64()
            );
        }
    }
}
