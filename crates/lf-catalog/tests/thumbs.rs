use std::time::{Duration, Instant};

use lf_catalog::Catalog;
use lf_catalog::import;
use lf_catalog::thumbs::{Priority, Thumbnailer};
use lf_pipeline::test_support::try_shared_gpu;
use lf_raw::demosaic::Quality;

fn wait_for(t: &Thumbnailer, n: usize) -> Vec<(i64, image::RgbaImage)> {
    let end = Instant::now() + Duration::from_secs(60);
    let mut got = Vec::new();
    while got.len() < n && Instant::now() < end {
        got.extend(t.take_ready());
        std::thread::sleep(Duration::from_millis(20));
    }
    got
}

fn setup(n: usize) -> (tempfile::TempDir, Vec<i64>) {
    let d = tempfile::tempdir().unwrap();
    for i in 0..n {
        image::RgbaImage::from_pixel(600, 400, image::Rgba([200, 90, 40, 255]))
            .save(d.path().join(format!("p{i}.png")))
            .unwrap();
    }
    let c = Catalog::open(d.path()).unwrap();
    let f = c.add_folder("f").unwrap();
    import::run(&c, f, &import::scan(d.path()), 200, &mut |_| Ok(())).unwrap();
    let mut ids = c.photo_ids().unwrap();
    ids.sort();
    (d, ids)
}

#[test]
fn makes_a_256px_jpeg_for_a_png() {
    let Some(gpu) = try_shared_gpu() else { return };
    let (d, ids) = setup(1);
    let t = Thumbnailer::new(gpu, d.path(), 1, Quality::Fast).unwrap();
    t.request(ids[0], Priority::Normal);
    let got = wait_for(&t, 1);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, ids[0]);
    assert_eq!(got[0].1.width().max(got[0].1.height()), 256);
    let c = Catalog::open(d.path()).unwrap();
    let (edge, jpeg) = c.thumbnail(ids[0]).unwrap().unwrap();
    assert_eq!(edge, 256);
    assert_eq!(&jpeg[..3], &[0xff, 0xd8, 0xff]);
}

#[test]
fn an_unreadable_photo_fails_visibly_and_makes_no_thumbnail() {
    let Some(gpu) = try_shared_gpu() else { return };
    let (d, ids) = setup(1);
    // Replace the picture with something that is not an image.
    std::fs::write(d.path().join("p0.png"), b"definitely not a png").unwrap();
    let t = Thumbnailer::new(gpu, d.path(), 1, Quality::Fast).unwrap();
    t.request(ids[0], Priority::High);
    let end = Instant::now() + Duration::from_secs(60);
    let mut failed = Vec::new();
    while failed.is_empty() && Instant::now() < end {
        failed.extend(t.take_failed());
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].0, ids[0]);
    assert!(t.take_ready().is_empty());
    let c = Catalog::open(d.path()).unwrap();
    assert!(c.thumbnail(ids[0]).unwrap().is_none());
    let (kind, err) = c
        .photo_kind_error(&d.path().join("p0.png").to_string_lossy())
        .unwrap();
    assert_eq!(kind, "png");
    assert!(err.is_some());
}

#[test]
fn high_priority_jumps_the_queue() {
    let Some(gpu) = try_shared_gpu() else { return };
    let (d, ids) = setup(4);
    let t = Thumbnailer::new_paused(gpu, d.path(), 1, Quality::Fast).unwrap();
    for id in &ids[..3] {
        t.request(*id, Priority::Normal);
    }
    t.request(ids[3], Priority::High);
    t.request(ids[3], Priority::High); // de-duplicated
    t.start();
    let got = wait_for(&t, 4);
    assert_eq!(got.len(), 4);
    assert_eq!(got[0].0, ids[3]);
}

#[test]
fn a_fresh_stored_thumbnail_is_served_without_decoding() {
    let Some(gpu) = try_shared_gpu() else { return };
    let (d, ids) = setup(1);
    {
        let t = Thumbnailer::new(gpu.clone(), d.path(), 1, Quality::Fast).unwrap();
        t.request(ids[0], Priority::High);
        assert_eq!(wait_for(&t, 1).len(), 1);
        assert_eq!(t.decoded_count(), 1);
    }
    let c = Catalog::open(d.path()).unwrap();
    assert!(c.fresh_thumbnail(ids[0]).unwrap().is_some());

    let t = Thumbnailer::new(gpu.clone(), d.path(), 1, Quality::Fast).unwrap();
    t.request(ids[0], Priority::High);
    let got = wait_for(&t, 1);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].1.width().max(got[0].1.height()), 256);
    assert_eq!(t.decoded_count(), 0, "served from the catalogue");
    drop(t);

    // An edit makes it stale: the next request decodes again.
    let mut s = lf_settings::EditSettings::default_settings();
    s.exposure = 0.5;
    lf_catalog::edits::set_settings(&c, ids[0], &s).unwrap();
    assert!(c.fresh_thumbnail(ids[0]).unwrap().is_none());
    let t = Thumbnailer::new(gpu, d.path(), 1, Quality::Fast).unwrap();
    t.request(ids[0], Priority::High);
    assert_eq!(wait_for(&t, 1).len(), 1);
    assert_eq!(t.decoded_count(), 1);
}

#[test]
fn a_success_clears_an_earlier_error() {
    let Some(gpu) = try_shared_gpu() else { return };
    let d = tempfile::tempdir().unwrap();
    let file = d.path().join("p0.png");
    std::fs::write(&file, b"not yet a png").unwrap();
    let c = Catalog::open(d.path()).unwrap();
    let f = c.add_folder("f").unwrap();
    import::run(&c, f, &import::scan(d.path()), 200, &mut |_| Ok(())).unwrap();
    let id = c.photo_ids().unwrap()[0];
    assert!(
        c.photo_kind_error(&file.to_string_lossy())
            .unwrap()
            .1
            .is_some()
    );
    // The file is fixed (a sync finished, a card re-copied): the thumbnail succeeds.
    image::RgbaImage::from_pixel(60, 40, image::Rgba([10, 20, 30, 255]))
        .save(&file)
        .unwrap();
    let t = Thumbnailer::new(gpu, d.path(), 1, Quality::Fast).unwrap();
    t.request(id, Priority::High);
    assert_eq!(wait_for(&t, 1).len(), 1);
    assert!(t.take_failed().is_empty());
    assert_eq!(c.photo_kind_error(&file.to_string_lossy()).unwrap().1, None);
}

#[test]
fn a_device_error_stops_the_workers_without_marking_the_photo() {
    let Some(gpu) = try_shared_gpu() else { return };
    let d = tempfile::tempdir().unwrap();
    image::RgbaImage::from_pixel(60, 40, image::Rgba([10, 20, 30, 255]))
        .save(d.path().join("lf-test-gpu-fail.png"))
        .unwrap();
    image::RgbaImage::from_pixel(60, 40, image::Rgba([10, 20, 30, 255]))
        .save(d.path().join("other.png"))
        .unwrap();
    let c = Catalog::open(d.path()).unwrap();
    let f = c.add_folder("f").unwrap();
    import::run(&c, f, &import::scan(d.path()), 200, &mut |_| Ok(())).unwrap();
    let ids = c.photo_ids().unwrap();
    let t = Thumbnailer::new_paused(gpu, d.path(), 1, Quality::Fast).unwrap();
    t.request(ids[0], Priority::High);
    t.request(ids[1], Priority::Normal);
    t.start();
    let end = Instant::now() + Duration::from_secs(60);
    let mut err = None;
    while err.is_none() && Instant::now() < end {
        err = t.take_device_error();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(err.is_some_and(|m| m.contains("injected")));
    assert!(t.take_device_error().is_none(), "reported once");
    std::thread::sleep(Duration::from_millis(300));
    assert!(t.take_failed().is_empty(), "not a broken photo");
    assert!(t.take_ready().is_empty(), "the workers stopped");
    let path = d.path().join("lf-test-gpu-fail.png");
    assert_eq!(c.photo_kind_error(&path.to_string_lossy()).unwrap().1, None);
}

#[test]
fn a_worker_that_cannot_open_the_catalogue_says_so() {
    let Some(gpu) = try_shared_gpu() else { return };
    let (d, _) = setup(1);
    let t = Thumbnailer::new_paused(gpu, d.path(), 1, Quality::Fast).unwrap();
    let conn = rusqlite::Connection::open(d.path().join(".lensflow/catalog.sqlite")).unwrap();
    conn.execute("INSERT INTO schema_version (version) VALUES (99)", [])
        .unwrap();
    drop(conn);
    t.start();
    let end = Instant::now() + Duration::from_secs(30);
    let mut failed = Vec::new();
    while failed.is_empty() && Instant::now() < end {
        failed.extend(t.take_failed());
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].0, 0);
    assert!(failed[0].1.contains("newer"), "{}", failed[0].1);
}

#[test]
fn cancel_pending_drops_queued_requests() {
    let Some(gpu) = try_shared_gpu() else { return };
    let (d, ids) = setup(3);
    let t = Thumbnailer::new_paused(gpu, d.path(), 1, Quality::Fast).unwrap();
    for id in &ids {
        t.request(*id, Priority::Normal);
    }
    t.cancel_pending();
    t.start();
    std::thread::sleep(Duration::from_millis(500));
    assert!(t.take_ready().is_empty());
    assert_eq!(t.decoded_count(), 0);
    // New requests still work.
    t.request(ids[0], Priority::High);
    assert_eq!(wait_for(&t, 1).len(), 1);
}
