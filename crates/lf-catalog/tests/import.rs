use std::fs;
use std::path::Path;
use std::time::Duration;

use lf_catalog::import::{self, ImportAbort, Progress};
use lf_catalog::{Catalog, edits, sidecar, versions};
use lf_settings::EditSettings;
use serde_json::json;

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
        .candidates
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
    let cands = import::scan(d.path()).candidates;
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
    let s = import::run(
        &c,
        folder,
        &import::scan(d.path()).candidates,
        200,
        &mut |_| Ok(()),
    )
    .unwrap();
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
    let s = import::run(
        &c,
        folder,
        &import::scan(d.path()).candidates,
        200,
        &mut |_| Ok(()),
    )
    .unwrap();
    assert_eq!((s.added, s.updated, s.skipped), (0, 1, 1));
    let (w, h) = c
        .photo_size(&d.path().join("a.png").to_string_lossy())
        .unwrap();
    assert_eq!((w, h), (Some(16), Some(10)));
}

fn only_id(c: &Catalog) -> i64 {
    let ids = c.photo_ids().unwrap();
    assert_eq!(ids.len(), 1);
    ids[0]
}

#[test]
fn import_adopts_a_v3_sidecar_and_keeps_its_unknown_keys_and_versions() {
    let d = tempfile::tempdir().unwrap();
    let photo = d.path().join("a.png");
    png(&photo, 8, 6);
    let mut settings = EditSettings::default_settings().to_value();
    settings["exposure"] = json!(0.75);
    let version = |id: &str, e: f64| {
        let mut s = EditSettings::default_settings().to_value();
        s["exposure"] = json!(e);
        json!({"id": id, "label": id, "settings": s, "created_at": "2026-01-01T00:00:00.000Z"})
    };
    let text = json!({
        "v": 3, "settings": settings, "updated_at": "2026-02-01T00:00:00.000Z",
        "versions": [version("first", 0.1), version("second", 0.2)],
        "future_feature": {"keep": [1, 2, 3]}
    });
    std::fs::write(sidecar::path_for(&photo), text.to_string()).unwrap();

    let c = Catalog::open(d.path()).unwrap();
    let folder = c.add_folder("f").unwrap();
    import::run(
        &c,
        folder,
        &import::scan(d.path()).candidates,
        200,
        &mut |_| Ok(()),
    )
    .unwrap();
    let id = only_id(&c);
    assert_eq!(edits::get_settings(&c, id).unwrap().unwrap().exposure, 0.75);
    assert_eq!(versions::list(&c, id).unwrap().len(), 2);

    // A later edit keeps the versions and the unknown keys.
    let mut s = EditSettings::default_settings();
    s.exposure = -0.5;
    let out = edits::set_settings(&c, id, &s).unwrap();
    assert_eq!(out.v, 3);
    assert_eq!(out.extra["future_feature"], json!({"keep": [1, 2, 3]}));
    assert_eq!(
        out.versions
            .iter()
            .map(|v| v.id.as_str())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    sidecar::write(&sidecar::path_for(&photo), &out).unwrap();
    let back = sidecar::read(&sidecar::path_for(&photo)).unwrap().unwrap();
    assert_eq!(back.versions.len(), 2);
    assert_eq!(back.extra["future_feature"], json!({"keep": [1, 2, 3]}));
}

#[test]
fn a_changed_file_with_a_newer_sidecar_takes_the_sidecar() {
    let d = tempfile::tempdir().unwrap();
    let photo = d.path().join("a.png");
    png(&photo, 8, 6);
    let c = Catalog::open(d.path()).unwrap();
    let folder = c.add_folder("f").unwrap();
    import::run(
        &c,
        folder,
        &import::scan(d.path()).candidates,
        200,
        &mut |_| Ok(()),
    )
    .unwrap();
    let id = only_id(&c);
    let mut s = EditSettings::default_settings();
    s.exposure = 0.2;
    let mut sc = edits::set_settings(&c, id, &s).unwrap();
    std::thread::sleep(Duration::from_millis(30));
    sc.settings.exposure = 0.9;
    sc.updated_at = lf_catalog::model::now();
    sidecar::write(&sidecar::path_for(&photo), &sc).unwrap();
    png(&photo, 16, 10);
    filetime::set_file_mtime(
        &photo,
        filetime::FileTime::from_system_time(
            std::time::SystemTime::now() + Duration::from_secs(60),
        ),
    )
    .unwrap();
    let r = import::run(
        &c,
        folder,
        &import::scan(d.path()).candidates,
        200,
        &mut |_| Ok(()),
    )
    .unwrap();
    assert_eq!(r.updated, 1);
    assert_eq!(edits::get_settings(&c, id).unwrap().unwrap().exposure, 0.9);
}

#[test]
fn a_corrupt_sidecar_is_reported_and_never_overwritten() {
    let d = tempfile::tempdir().unwrap();
    let photo = d.path().join("a.png");
    png(&photo, 8, 6);
    let side = sidecar::path_for(&photo);
    let corrupt = b"{\"v\": 2, \"settings\": {\"exposure\": 0.5"; // cut short
    std::fs::write(&side, corrupt).unwrap();
    let c = Catalog::open(d.path()).unwrap();
    let folder = c.add_folder("f").unwrap();
    import::run(
        &c,
        folder,
        &import::scan(d.path()).candidates,
        200,
        &mut |_| Ok(()),
    )
    .unwrap();
    let (kind, err) = c.photo_kind_error(&photo.to_string_lossy()).unwrap();
    assert_eq!(kind, "png");
    assert_eq!(err, None, "the photo itself is fine");
    let id = only_id(&c);
    let row = c.photo(id).unwrap().unwrap();
    let err = row.sidecar_error.expect("the damaged sidecar is reported");
    assert!(err.contains("a.png.lensflow.json"), "{err}");
    assert!(err.contains("left untouched"), "{err}");
    assert_eq!(std::fs::read(&side).unwrap(), corrupt);

    // An edit does not get to replace it either.
    let out = edits::set_settings(&c, id, &EditSettings::default_settings()).unwrap();
    assert!(sidecar::write(&side, &out).is_err());
    assert_eq!(std::fs::read(&side).unwrap(), corrupt);
}

#[test]
fn a_decoder_panic_marks_the_file_unreadable_and_the_import_finishes() {
    let d = tempfile::tempdir().unwrap();
    png(&d.path().join("a.png"), 8, 6);
    png(&d.path().join("lf-test-panic.png"), 8, 6);
    png(&d.path().join("z.png"), 8, 6);
    let c = Catalog::open(d.path()).unwrap();
    let folder = c.add_folder("f").unwrap();
    let s = import::run(
        &c,
        folder,
        &import::scan(d.path()).candidates,
        200,
        &mut |_| Ok(()),
    )
    .unwrap();
    assert_eq!(s.added, 3);
    let (kind, err) = c
        .photo_kind_error(&d.path().join("lf-test-panic.png").to_string_lossy())
        .unwrap();
    assert_eq!(kind, "unreadable");
    assert_eq!(err.as_deref(), Some("the decoder crashed on this file"));
    let (kind, _) = c
        .photo_kind_error(&d.path().join("z.png").to_string_lossy())
        .unwrap();
    assert_eq!(kind, "png");
}

#[test]
fn mark_missing_flags_gone_files_and_run_clears_the_flag() {
    let d = tempfile::tempdir().unwrap();
    let (a, b) = (d.path().join("a.png"), d.path().join("b.png"));
    png(&a, 8, 6);
    png(&b, 8, 6);
    let c = Catalog::open(d.path()).unwrap();
    let folder = c.add_folder("f").unwrap();
    let other = c.add_folder("g").unwrap();
    import::run(
        &c,
        folder,
        &import::scan(d.path()).candidates,
        200,
        &mut |_| Ok(()),
    )
    .unwrap();
    let elsewhere = d.path().join("elsewhere.png");
    c.add_photo(
        other,
        &lf_catalog::NewPhoto {
            path: elsewhere.to_string_lossy().into_owned(),
            kind: "png".into(),
            ..Default::default()
        },
    )
    .unwrap();

    let moved = d.path().join("b.moved");
    std::fs::rename(&b, &moved).unwrap();
    let scanned = import::scan(d.path());
    assert_eq!(import::mark_missing(&c, folder, &scanned).unwrap(), 1);
    assert!(c.photo_missing(&b.to_string_lossy()).unwrap());
    assert!(!c.photo_missing(&a.to_string_lossy()).unwrap());
    assert!(
        !c.photo_missing(&elsewhere.to_string_lossy()).unwrap(),
        "another folder's rows are not touched"
    );
    // Already flagged: not counted again.
    assert_eq!(import::mark_missing(&c, folder, &scanned).unwrap(), 0);

    std::fs::rename(&moved, &b).unwrap();
    let s = import::run(
        &c,
        folder,
        &import::scan(d.path()).candidates,
        200,
        &mut |_| Ok(()),
    )
    .unwrap();
    assert_eq!(s.skipped, 2);
    assert!(!c.photo_missing(&b.to_string_lossy()).unwrap());
}

#[test]
fn an_unreadable_photo_and_a_damaged_sidecar_are_reported_separately() {
    let d = tempfile::tempdir().unwrap();
    let photo = d.path().join("a.png");
    std::fs::write(&photo, b"not a png").unwrap();
    std::fs::write(sidecar::path_for(&photo), b"{ cut short").unwrap();
    let c = Catalog::open(d.path()).unwrap();
    let folder = c.add_folder("f").unwrap();
    import::run(
        &c,
        folder,
        &import::scan(d.path()).candidates,
        200,
        &mut |_| Ok(()),
    )
    .unwrap();
    let row = c.photo(only_id(&c)).unwrap().unwrap();
    assert!(row.error.is_some(), "the file's own problem");
    assert!(row.sidecar_error.is_some(), "and the sidecar's");
}

#[test]
fn a_repaired_sidecar_clears_sidecar_error_on_reconcile() {
    let d = tempfile::tempdir().unwrap();
    let photo = d.path().join("a.png");
    png(&photo, 8, 6);
    let side = sidecar::path_for(&photo);
    std::fs::write(&side, b"{ cut short").unwrap();
    let c = Catalog::open(d.path()).unwrap();
    let folder = c.add_folder("f").unwrap();
    import::run(
        &c,
        folder,
        &import::scan(d.path()).candidates,
        200,
        &mut |_| Ok(()),
    )
    .unwrap();
    let id = only_id(&c);
    assert!(c.photo(id).unwrap().unwrap().sidecar_error.is_some());

    std::fs::remove_file(&side).unwrap();
    edits::reconcile(&c, id, None).unwrap();
    assert_eq!(c.photo(id).unwrap().unwrap().sidecar_error, None);
}

#[test]
fn scan_reports_whether_the_root_was_readable() {
    let d = tempfile::tempdir().unwrap();
    png(&d.path().join("a.png"), 8, 6);
    let found = import::scan(d.path());
    assert!(found.root_readable);
    assert_eq!(found.candidates.len(), 1);

    let gone = import::scan(&d.path().join("unplugged"));
    assert!(!gone.root_readable);
    assert!(gone.candidates.is_empty());
}

#[test]
fn mark_missing_marks_nothing_when_the_folder_was_not_reached() {
    let d = tempfile::tempdir().unwrap();
    let shoot = d.path().join("shoot");
    std::fs::create_dir(&shoot).unwrap();
    png(&shoot.join("a.png"), 8, 6);
    png(&shoot.join("b.png"), 8, 6);
    let c = Catalog::open(d.path()).unwrap();
    let folder = c.add_folder(&shoot.to_string_lossy()).unwrap();
    import::run(
        &c,
        folder,
        &import::scan(&shoot).candidates,
        200,
        &mut |_| Ok(()),
    )
    .unwrap();

    // The drive is unplugged: the folder cannot be read at all.
    std::fs::rename(&shoot, d.path().join("shoot.away")).unwrap();
    let unreachable = import::scan(&shoot);
    assert_eq!(import::mark_missing(&c, folder, &unreachable).unwrap(), 0);

    // An empty mount point left behind by an unplugged drive: readable, but nothing in it.
    std::fs::create_dir(&shoot).unwrap();
    let empty = import::scan(&shoot);
    assert!(empty.root_readable);
    assert_eq!(import::mark_missing(&c, folder, &empty).unwrap(), 0);
    for row in c.photos_in(folder).unwrap() {
        assert!(!row.missing, "{}", row.path.display());
    }
}

#[test]
fn mark_missing_confirmed_flags_every_photo_of_the_folder() {
    let d = tempfile::tempdir().unwrap();
    let shoot = d.path().join("shoot");
    std::fs::create_dir(&shoot).unwrap();
    png(&shoot.join("a.png"), 8, 6);
    png(&shoot.join("b.png"), 8, 6);
    let c = Catalog::open(d.path()).unwrap();
    let folder = c.add_folder(&shoot.to_string_lossy()).unwrap();
    let other = c.add_folder("other").unwrap();
    c.add_photo(
        other,
        &lf_catalog::NewPhoto {
            path: "other/x.png".into(),
            kind: "png".into(),
            ..Default::default()
        },
    )
    .unwrap();
    import::run(
        &c,
        folder,
        &import::scan(&shoot).candidates,
        200,
        &mut |_| Ok(()),
    )
    .unwrap();
    assert_eq!(import::present_count(&c, folder).unwrap(), 2);

    // The folder was emptied on purpose and the user said so.
    assert_eq!(import::mark_missing_confirmed(&c, folder).unwrap(), 2);
    assert!(c.photos_in(folder).unwrap().iter().all(|r| r.missing));
    assert_eq!(import::present_count(&c, folder).unwrap(), 0);
    assert!(
        !c.photo_missing("other/x.png").unwrap(),
        "other folders untouched"
    );
    assert_eq!(import::mark_missing_confirmed(&c, folder).unwrap(), 0);
}
