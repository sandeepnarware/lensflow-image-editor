use std::fs;
use std::path::Path;
use std::time::Duration;

use lf_catalog::Catalog;
use lf_catalog::import::{self, ImportAbort, Progress};

fn png(path: &Path, w: u32, h: u32) {
    image::RgbaImage::from_pixel(w, h, image::Rgba([90, 120, 200, 255]))
        .save(path)
        .unwrap();
}

fn count(c: &Catalog) -> i64 {
    c.photo_count().unwrap()
}

#[test]
fn scan_filters_by_extension_and_skips_hidden() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    fs::create_dir_all(r.join("sub/deep")).unwrap();
    fs::create_dir_all(r.join(".hidden")).unwrap();
    fs::create_dir_all(r.join(".lensflow")).unwrap();
    for f in [
        "a.JPG",
        "b.nef",
        "sub/c.png",
        "sub/deep/d.DnG",
        "notes.txt",
        "x.lensflow.json",
        ".dot.jpg",
        ".hidden/e.jpg",
        ".lensflow/f.jpg",
        "sub/g.mov",
    ] {
        fs::write(r.join(f), b"x").unwrap();
    }
    let mut names: Vec<String> = import::scan(r)
        .iter()
        .map(|c| {
            c.path
                .strip_prefix(r)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    names.sort();
    assert_eq!(names, ["a.JPG", "b.nef", "sub/c.png", "sub/deep/d.DnG"]);
}

fn thousand(root: &Path) {
    for i in 0..1000 {
        png(&root.join(format!("p{i:04}.png")), 8, 6);
    }
}

#[test]
fn fault_after_batch_two_leaves_400_then_rerun_completes() {
    let d = tempfile::tempdir().unwrap();
    thousand(d.path());
    let c = Catalog::open(d.path()).unwrap();
    let folder = c.add_folder(&d.path().to_string_lossy()).unwrap();
    let cands = import::scan(d.path());
    assert_eq!(cands.len(), 1000);
    let mut seen = Vec::new();
    let r = import::run(&c, folder, &cands, 200, &mut |p: Progress| {
        seen.push(p.done);
        if p.done >= 400 {
            Err(ImportAbort)
        } else {
            Ok(())
        }
    });
    assert!(r.is_err());
    assert_eq!(seen.last(), Some(&400));
    assert!(seen.len() > 2, "progress inside batches: {seen:?}");
    assert!(seen.windows(2).all(|w| w[1] - w[0] <= 25), "{seen:?}");
    assert_eq!(count(&c), 400);

    let s = import::run(&c, folder, &cands, 200, &mut |_| Ok(())).unwrap();
    assert_eq!((s.added, s.updated, s.skipped), (600, 0, 400));
    assert_eq!(count(&c), 1000);
    let s = import::run(&c, folder, &cands, 200, &mut |_| Ok(())).unwrap();
    assert_eq!((s.added, s.updated, s.skipped), (0, 0, 1000));
}

#[test]
fn changed_files_update_and_bad_files_are_recorded() {
    let d = tempfile::tempdir().unwrap();
    png(&d.path().join("a.png"), 8, 6);
    fs::write(d.path().join("bad.png"), b"not a png").unwrap();
    let c = Catalog::open(d.path()).unwrap();
    let folder = c.add_folder("f").unwrap();
    let s = import::run(&c, folder, &import::scan(d.path()), 200, &mut |_| Ok(())).unwrap();
    assert_eq!((s.added, s.updated, s.skipped), (2, 0, 0));
    let (kind, err) = c
        .photo_kind_error(&d.path().join("bad.png").to_string_lossy())
        .unwrap();
    assert_eq!(kind, "unreadable");
    assert!(err.is_some());
    let (w, h) = c
        .photo_size(&d.path().join("a.png").to_string_lossy())
        .unwrap();
    assert_eq!((w, h), (Some(8), Some(6)));

    png(&d.path().join("a.png"), 16, 10);
    filetime::set_file_mtime(
        d.path().join("a.png"),
        filetime::FileTime::from_system_time(
            std::time::SystemTime::now() + Duration::from_secs(60),
        ),
    )
    .unwrap();
    let s = import::run(&c, folder, &import::scan(d.path()), 200, &mut |_| Ok(())).unwrap();
    assert_eq!((s.added, s.updated, s.skipped), (0, 1, 1));
    let (w, h) = c
        .photo_size(&d.path().join("a.png").to_string_lossy())
        .unwrap();
    assert_eq!((w, h), (Some(16), Some(10)));
}
