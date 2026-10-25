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
