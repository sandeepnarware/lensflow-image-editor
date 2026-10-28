mod support {
    pub mod synthetic_dng;
}

use std::ops::ControlFlow;
use std::path::Path;

use image::{ImageReader, Rgb, Rgb32FImage, RgbaImage};
use lf_catalog::export::{self, Destination, ExportSpec, Format, Metadata, Pattern, Size};
use lf_catalog::{Catalog, CatalogError, edits, import};
use lf_pipeline::test_support::try_shared_gpu;
use lf_raw::color::XYZ_D65_TO_LINEAR_SRGB;
use lf_raw::exif::{ExifSummary, extract_jpeg_exif, read_summary};
use lf_settings::EditSettings;
use lf_settings::geometry::{Geometry, output_size};

fn spec(format: Format, dest: &Path) -> ExportSpec {
    ExportSpec {
        format,
        size: Size::Original,
        destination: Destination::Folder(dest.to_path_buf()),
        rename: Pattern("{name}".to_string()),
        metadata: Metadata::None,
        overwrite: false,
        sharpen: Default::default(),
        watermark: None,
        space: Default::default(),
    }
}

fn catalogue(dir: &Path) -> (Catalog, Vec<i64>) {
    let c = Catalog::open(dir).unwrap();
    let f = c.add_folder("f").unwrap();
    import::run(&c, f, &import::scan(dir).candidates, 200, &mut |_| Ok(())).unwrap();
    let ids = c.photo_ids().unwrap();
    (c, ids)
}

fn png(path: &Path, w: u32, h: u32) {
    RgbaImage::from_fn(w, h, |x, y| {
        image::Rgba([(x * 2) as u8, (y * 3) as u8, 90, 255])
    })
    .save(path)
    .unwrap();
}

/// A big-endian Exif APP1 holding only Make = "Cam".
fn app1_make_cam() -> Vec<u8> {
    let mut tiff = b"MM\0\x2a\0\0\0\x08".to_vec();
    tiff.extend([0, 1]); // one entry
    tiff.extend([0x01, 0x0f, 0, 2, 0, 0, 0, 4]);
    tiff.extend(b"Cam\0");
    tiff.extend([0, 0, 0, 0]);
    let mut seg = vec![0xff, 0xe1];
    seg.extend(((2 + 6 + tiff.len()) as u16).to_be_bytes());
    seg.extend(b"Exif\0\0");
    seg.extend(tiff);
    seg
}

fn jpeg_with_exif(path: &Path) {
    let img = image::RgbImage::from_fn(64, 48, |x, y| Rgb([(x * 4) as u8, (y * 5) as u8, 60]));
    let mut buf = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 90)
        .encode_image(&img)
        .unwrap();
    let app1 = app1_make_cam();
    let mut out = buf[..2].to_vec();
    out.extend(&app1);
    out.extend(&buf[2..]);
    std::fs::write(path, out).unwrap();
}

#[test]
fn jpeg_of_a_png_has_the_pipelines_output_size() {
    let Some(gpu) = try_shared_gpu() else { return };
    let src = tempfile::tempdir().unwrap();
    png(&src.path().join("a.png"), 120, 80);
    let (c, ids) = catalogue(src.path());
    let mut s = EditSettings::default_settings();
    s.crop.w = 0.5;
    edits::set_settings(&c, ids[0], &s).unwrap();
    let out = tempfile::tempdir().unwrap();
    let mut seen = Vec::new();
    let done = export::run(
        gpu,
        &c,
        &ids,
        &spec(Format::Jpeg { quality: 90 }, out.path()),
        &mut |d, t| {
            seen.push((d, t));
            ControlFlow::Continue(())
        },
    )
    .unwrap();
    let (w, h) = output_size(120, 80, &Geometry::from(&s));
    assert_eq!((done.exported[0].width, done.exported[0].height), (w, h));
    assert_eq!(
        image::image_dimensions(&done.exported[0].path).unwrap(),
        (w, h)
    );
    assert_eq!(done.exported[0].path, out.path().join("a.jpg"));
    assert_eq!(seen.last(), Some(&(1, 1)));
    let job = export::last_job(&c).unwrap().unwrap();
    assert_eq!((job.status.as_str(), job.done, job.total), ("done", 1, 1));
}

#[test]
fn size_long_edge_resizes() {
    let Some(gpu) = try_shared_gpu() else { return };
    let src = tempfile::tempdir().unwrap();
    png(&src.path().join("a.png"), 120, 80);
    let (c, ids) = catalogue(src.path());
    let out = tempfile::tempdir().unwrap();
    let mut sp = spec(Format::Png, out.path());
    sp.size = Size::LongEdge(60);
    let done = export::run(gpu, &c, &ids, &sp, &mut |_, _| ControlFlow::Continue(())).unwrap();
    assert_eq!(
        image::image_dimensions(&done.exported[0].path).unwrap(),
        (60, 40)
    );
    assert_eq!((done.exported[0].width, done.exported[0].height), (60, 40));
}

#[test]
fn a_jpeg_sources_app1_is_carried_byte_for_byte() {
    let Some(gpu) = try_shared_gpu() else { return };
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("j.jpg");
    jpeg_with_exif(&file);
    let kept = extract_jpeg_exif(&std::fs::read(&file).unwrap()).unwrap();
    let (c, ids) = catalogue(src.path());
    let out = tempfile::tempdir().unwrap();
    let mut sp = spec(Format::Jpeg { quality: 85 }, out.path());
    sp.metadata = Metadata::All;
    let done = export::run(gpu, &c, &ids, &sp, &mut |_, _| ControlFlow::Continue(())).unwrap();
    let bytes = std::fs::read(&done.exported[0].path).unwrap();
    // Straight after the encoder's JFIF header (18 bytes), as the web's `insertExif` does.
    assert_eq!(&bytes[2..4], &[0xff, 0xe0]);
    assert_eq!(&bytes[20..20 + kept.len()], kept.as_slice());
    assert_eq!(extract_jpeg_exif(&bytes).unwrap(), kept);
    // And exactly one APP1.
    assert_eq!(bytes.windows(6).filter(|w| *w == b"Exif\0\0").count(), 1);
    assert_eq!(read_summary(&bytes).make.as_deref(), Some("Cam"));
}

fn dng(path: &Path) {
    let linear = Rgb32FImage::from_fn(64, 48, |x, y| {
        let v = if (40..56).contains(&x) && (16..32).contains(&y) {
            3.0
        } else {
            0.02 + 0.5 * (x as f32 / 63.0) * (0.3 + y as f32 / 70.0)
        };
        Rgb([v, v, v])
    });
    support::synthetic_dng::write(
        path,
        &linear,
        support::synthetic_dng::RGGB,
        64,
        16383,
        [2.0, 1.0, 1.5],
        XYZ_D65_TO_LINEAR_SRGB,
    );
}

#[test]
fn tiff16_keeps_16_bit_precision_and_clamps_only_at_the_encode() {
    let Some(gpu) = try_shared_gpu() else { return };
    let src = tempfile::tempdir().unwrap();
    dng(&src.path().join("r.dng"));
    let (c, ids) = catalogue(src.path());
    let out = tempfile::tempdir().unwrap();
    let done = export::run(
        gpu,
        &c,
        &ids,
        &spec(Format::Tiff16, out.path()),
        &mut |_, _| ControlFlow::Continue(()),
    )
    .unwrap();
    assert_eq!(done.exported[0].path.extension().unwrap(), "tif");
    let img = ImageReader::open(&done.exported[0].path)
        .unwrap()
        .decode()
        .unwrap();
    let img = img.into_rgb16();
    assert_eq!(img.dimensions(), (64, 48));
    let off_grid = img.pixels().filter(|p| p.0[0] % 257 != 0).count();
    assert!(off_grid > 500, "only {off_grid} values off the 8-bit grid");
    // The above-1 patch clamps at 1.0 (65535), no higher, no wrap.
    assert_eq!(img.get_pixel(48, 24).0, [65535; 3]);
}

#[test]
fn raw_exports_get_a_minimal_exif_from_the_summary() {
    let Some(gpu) = try_shared_gpu() else { return };
    let src = tempfile::tempdir().unwrap();
    dng(&src.path().join("r.dng"));
    let (c, ids) = catalogue(src.path());
    for meta in [Metadata::All, Metadata::None] {
        let out = tempfile::tempdir().unwrap();
        let mut sp = spec(Format::Jpeg { quality: 80 }, out.path());
        sp.metadata = meta;
        let done = export::run(gpu.clone(), &c, &ids, &sp, &mut |_, _| {
            ControlFlow::Continue(())
        })
        .unwrap();
        let s = read_summary(&std::fs::read(&done.exported[0].path).unwrap());
        match meta {
            Metadata::All => {
                assert_eq!(s.make.as_deref(), Some("Synthetic"));
                assert_eq!(s.orientation, 1);
            }
            _ => assert_eq!(s.make, None),
        }
    }
}

#[test]
fn the_exif_writer_round_trips_every_tag() {
    use lf_catalog::exif_embed::{build_exif, insert_exif};
    let summary = ExifSummary {
        make: Some("Canon".into()),
        model: Some("EOS R6".into()),
        lens: Some("RF 24-70mm".into()),
        iso: Some(800),
        exposure_time: Some(0.004),
        f_number: Some(2.8),
        focal_length: Some(50.0),
        captured_at: Some("2024-05-01T12:34:56".into()),
        orientation: 6,
        artist: Some("Asha".into()),
        copyright: Some("(c) Asha".into()),
    };
    let app1 = build_exif(&summary, false).unwrap();
    let jpeg = insert_exif(&[0xff, 0xd8, 0xff, 0xd9], &app1);
    let got = read_summary(&jpeg);
    assert_eq!(got.make, summary.make);
    assert_eq!(got.model, summary.model);
    assert_eq!(got.lens, summary.lens);
    assert_eq!(got.iso, Some(800));
    assert!((got.exposure_time.unwrap() - 0.004).abs() < 1e-5);
    assert!((got.f_number.unwrap() - 2.8).abs() < 1e-3);
    assert_eq!(got.focal_length, Some(50.0));
    assert_eq!(got.captured_at, summary.captured_at);
    assert_eq!(got.orientation, 1);
    assert_eq!(got.artist, summary.artist);
    assert_eq!(got.copyright, summary.copyright);

    let only = build_exif(&summary, true).unwrap();
    let got = read_summary(&insert_exif(&[0xff, 0xd8, 0xff, 0xd9], &only));
    assert_eq!(
        (got.artist, got.copyright),
        (summary.artist, summary.copyright)
    );
    assert_eq!(got.make, None);
    assert!(build_exif(&ExifSummary::default(), true).is_none());
}

#[test]
fn insert_exif_drops_an_encoder_app1_and_goes_after_soi() {
    use lf_catalog::exif_embed::{build_exif, insert_exif};
    let old = app1_make_cam();
    let mut jpeg = vec![0xff, 0xd8];
    jpeg.extend(&old);
    jpeg.extend([0xff, 0xd9]);
    let new = build_exif(
        &ExifSummary {
            make: Some("X".into()),
            ..Default::default()
        },
        false,
    )
    .unwrap();
    let out = insert_exif(&jpeg, &new);
    assert_eq!(&out[..2], &[0xff, 0xd8]);
    assert_eq!(&out[2..2 + new.len()], new.as_slice());
    assert_eq!(out.len(), 2 + new.len() + 2);
}

#[test]
fn rename_tokens() {
    use export::file_name;
    assert_eq!(
        file_name(
            "{date}_{name}_{seq}",
            "IMG 1",
            3,
            12,
            "2024-05-01",
            "Canon EOS R6"
        ),
        "2024-05-01_IMG 1_03"
    );
    assert_eq!(
        file_name("{camera}-{seq}", "a", 1, 1, "d", "Canon EOS R6"),
        "Canon-EOS-R6-1"
    );
    assert_eq!(file_name("{camera}", "a", 1, 1, "d", ""), "unknown");
    assert_eq!(file_name("{name}:x/y?", "a", 1, 1, "d", ""), "a-x-y-");
    assert_eq!(file_name("{seq}", "a", 7, 100, "d", ""), "007");
}

#[test]
fn rename_applies_to_the_file_and_date_falls_back_to_mtime() {
    let Some(gpu) = try_shared_gpu() else { return };
    let src = tempfile::tempdir().unwrap();
    png(&src.path().join("a.png"), 32, 32);
    png(&src.path().join("b.png"), 32, 32);
    let (c, ids) = catalogue(src.path());
    let out = tempfile::tempdir().unwrap();
    let mut sp = spec(Format::Png, out.path());
    sp.rename = Pattern("{seq}_{name}_{date}".into());
    let done = export::run(gpu, &c, &ids, &sp, &mut |_, _| ControlFlow::Continue(())).unwrap();
    let names: Vec<String> = done
        .exported
        .iter()
        .map(|d| d.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert!(
        names[0].starts_with("1_a_") && names[0].ends_with(".png"),
        "{names:?}"
    );
    assert!(names[1].starts_with("2_b_"), "{names:?}");
    let date = &names[0][4..names[0].len() - 4];
    assert!(
        chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok(),
        "{date}"
    );
}

#[test]
fn overwrite_false_refuses_an_existing_path() {
    let Some(gpu) = try_shared_gpu() else { return };
    let src = tempfile::tempdir().unwrap();
    png(&src.path().join("a.png"), 32, 32);
    let (c, ids) = catalogue(src.path());
    let out = tempfile::tempdir().unwrap();
    let sp = spec(Format::Png, out.path());
    export::run(gpu.clone(), &c, &ids, &sp, &mut |_, _| {
        ControlFlow::Continue(())
    })
    .unwrap();
    let before = std::fs::read(out.path().join("a.png")).unwrap();
    let err = export::run(gpu.clone(), &c, &ids, &sp, &mut |_, _| {
        ControlFlow::Continue(())
    })
    .unwrap_err();
    match err {
        CatalogError::Exists(p) => assert_eq!(p, out.path().join("a.png")),
        e => panic!("{e}"),
    }
    assert_eq!(std::fs::read(out.path().join("a.png")).unwrap(), before);
    let job = export::last_job(&c).unwrap().unwrap();
    assert_eq!(job.status, "failed");
    assert!(job.error.unwrap().contains("a.png"));
    let mut again = sp.clone();
    again.overwrite = true;
    export::run(gpu, &c, &ids, &again, &mut |_, _| ControlFlow::Continue(())).unwrap();
}

#[test]
fn two_photos_on_one_name_fail_before_anything_is_written() {
    let Some(gpu) = try_shared_gpu() else { return };
    let src = tempfile::tempdir().unwrap();
    png(&src.path().join("a.png"), 32, 32);
    jpeg_with_exif(&src.path().join("a.jpg"));
    let (c, ids) = catalogue(src.path());
    for overwrite in [true, false] {
        let out = tempfile::tempdir().unwrap();
        let mut sp = spec(Format::Png, out.path());
        sp.overwrite = overwrite;
        let err = export::run(gpu.clone(), &c, &ids, &sp, &mut |_, _| {
            ControlFlow::Continue(())
        })
        .unwrap_err();
        match &err {
            CatalogError::NameCollision { path, photos } => {
                assert_eq!(path, &out.path().join("a.png"));
                assert_ne!(photos.0, photos.1);
                assert!(err.to_string().contains("{seq}"), "{err}");
            }
            e => panic!("{e}"),
        }
        assert_eq!(std::fs::read_dir(out.path()).unwrap().count(), 0);
        assert_eq!(export::last_job(&c).unwrap().unwrap().status, "failed");
    }
    // {seq} resolves it.
    let out = tempfile::tempdir().unwrap();
    let mut sp = spec(Format::Png, out.path());
    sp.rename = Pattern("{name}-{seq}".into());
    assert_eq!(
        export::run(gpu, &c, &ids, &sp, &mut |_, _| ControlFlow::Continue(()))
            .unwrap()
            .exported
            .len(),
        2
    );
}

#[test]
fn an_existing_file_anywhere_in_the_batch_stops_the_export_before_it_starts() {
    let Some(gpu) = try_shared_gpu() else { return };
    let src = tempfile::tempdir().unwrap();
    png(&src.path().join("a.png"), 32, 32);
    png(&src.path().join("b.png"), 32, 32);
    let (c, ids) = catalogue(src.path());
    let out = tempfile::tempdir().unwrap();
    std::fs::write(out.path().join("b.png"), b"mine").unwrap();
    let err = export::run(
        gpu,
        &c,
        &ids,
        &spec(Format::Png, out.path()),
        &mut |_, _| ControlFlow::Continue(()),
    )
    .unwrap_err();
    assert!(matches!(err, CatalogError::Exists(p) if p == out.path().join("b.png")));
    assert!(!out.path().join("a.png").exists());
    assert_eq!(std::fs::read(out.path().join("b.png")).unwrap(), b"mine");
}

#[test]
fn windows_reserved_names_and_trailing_dots_are_made_safe() {
    use export::{file_name, safe_stem};
    assert_eq!(safe_stem("CON"), "CON_");
    assert_eq!(safe_stem("com1"), "com1_");
    assert_eq!(safe_stem("Lpt9.final"), "Lpt9.final_");
    assert_eq!(safe_stem("COM10"), "COM10");
    assert_eq!(safe_stem("console"), "console");
    assert_eq!(safe_stem("shot. . "), "shot");
    assert_eq!(file_name("{name}. ", "a", 1, 1, "d", ""), "a");
}

#[test]
fn the_exif_writer_adds_datetime_and_exif_version() {
    use lf_catalog::exif_embed::{build_exif, insert_exif};
    let app1 = build_exif(
        &ExifSummary {
            make: Some("X".into()),
            captured_at: Some("2024-05-01T12:34:56".into()),
            orientation: 1,
            ..Default::default()
        },
        false,
    )
    .unwrap();
    let jpeg = insert_exif(&[0xff, 0xd8, 0xff, 0xd9], &app1);
    let e = exif::Reader::new()
        .read_from_container(&mut std::io::Cursor::new(&jpeg))
        .unwrap();
    let dt = e.get_field(exif::Tag::DateTime, exif::In::PRIMARY).unwrap();
    assert_eq!(dt.display_value().to_string(), "2024-05-01 12:34:56");
    let v = e
        .get_field(exif::Tag::ExifVersion, exif::In::PRIMARY)
        .unwrap();
    assert!(matches!(&v.value, exif::Value::Undefined(b, _) if b == b"0232"));
}

#[test]
fn very_long_text_is_cut_instead_of_dropping_the_exif() {
    use lf_catalog::exif_embed::build_exif;
    let long = "é".repeat(40_000);
    let app1 = build_exif(
        &ExifSummary {
            make: Some("Canon".into()),
            copyright: Some(long),
            ..Default::default()
        },
        false,
    )
    .unwrap();
    assert!(app1.len() < 65_536);
}

#[test]
fn one_bad_photo_in_the_middle_does_not_stop_the_rest() {
    let Some(gpu) = try_shared_gpu() else { return };
    let src = tempfile::tempdir().unwrap();
    for n in ["a", "b", "c"] {
        png(&src.path().join(format!("{n}.png")), 32, 32);
    }
    let (c, ids) = catalogue(src.path());
    std::fs::write(src.path().join("b.png"), b"no longer a png").unwrap();
    let out = tempfile::tempdir().unwrap();
    let mut seen = Vec::new();
    let report = export::run(
        gpu,
        &c,
        &ids,
        &spec(Format::Png, out.path()),
        &mut |d, t| {
            seen.push((d, t));
            ControlFlow::Continue(())
        },
    )
    .unwrap();
    assert_eq!(report.exported.len(), 2);
    assert_eq!(report.failed.len(), 1);
    assert_eq!(report.failed[0].0, ids[1]);
    assert!(!report.cancelled);
    assert!(out.path().join("a.png").exists() && out.path().join("c.png").exists());
    assert!(!out.path().join("b.png").exists());
    assert_eq!(seen, [(0, 3), (1, 3), (2, 3), (3, 3)]);
    let job = export::last_job(&c).unwrap().unwrap();
    assert_eq!((job.status.as_str(), job.done, job.total), ("failed", 2, 3));
    assert!(job.error.unwrap().contains("1 of 3"));
}

#[test]
fn a_progress_break_cancels_after_the_current_photo() {
    let Some(gpu) = try_shared_gpu() else { return };
    let src = tempfile::tempdir().unwrap();
    for n in ["a", "b", "c"] {
        png(&src.path().join(format!("{n}.png")), 32, 32);
    }
    let (c, ids) = catalogue(src.path());
    let out = tempfile::tempdir().unwrap();
    let report = export::run(
        gpu,
        &c,
        &ids,
        &spec(Format::Png, out.path()),
        &mut |d, _| {
            if d >= 1 {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        },
    )
    .unwrap();
    assert!(report.cancelled);
    assert_eq!(report.exported.len(), 1);
    assert!(report.failed.is_empty());
    assert_eq!(std::fs::read_dir(out.path()).unwrap().count(), 1);
    let job = export::last_job(&c).unwrap().unwrap();
    assert_eq!((job.status.as_str(), job.done), ("cancelled", 1));
}

#[test]
fn a_decoder_panic_fails_that_photo_only() {
    let Some(gpu) = try_shared_gpu() else { return };
    let src = tempfile::tempdir().unwrap();
    png(&src.path().join("a.png"), 32, 32);
    png(&src.path().join("lf-test-panic.png"), 32, 32);
    let (c, ids) = catalogue(src.path());
    let out = tempfile::tempdir().unwrap();
    let report = export::run(
        gpu,
        &c,
        &ids,
        &spec(Format::Png, out.path()),
        &mut |_, _| ControlFlow::Continue(()),
    )
    .unwrap();
    assert_eq!(report.exported.len(), 1);
    assert_eq!(report.failed.len(), 1);
    assert!(
        report.failed[0]
            .1
            .to_string()
            .contains("the decoder crashed on this file"),
        "{}",
        report.failed[0].1
    );
}

#[test]
fn a_reserved_name_from_the_pattern_itself_is_made_safe() {
    let Some(gpu) = try_shared_gpu() else { return };
    let src = tempfile::tempdir().unwrap();
    png(&src.path().join("a.png"), 32, 32);
    let (c, ids) = catalogue(src.path());
    let out = tempfile::tempdir().unwrap();
    let mut sp = spec(Format::Png, out.path());
    sp.rename = Pattern("CON".into());
    let report = export::run(gpu, &c, &ids, &sp, &mut |_, _| ControlFlow::Continue(())).unwrap();
    assert_eq!(report.exported[0].path, out.path().join("CON_.png"));
}

#[test]
fn a_percent_size_beyond_the_device_limit_is_refused_for_that_photo() {
    let Some(gpu) = try_shared_gpu() else { return };
    let src = tempfile::tempdir().unwrap();
    png(&src.path().join("a.png"), 32, 32);
    let (c, ids) = catalogue(src.path());
    let out = tempfile::tempdir().unwrap();
    let mut sp = spec(Format::Png, out.path());
    let factor = (gpu.max_edge() as f32 / 32.0 + 1.0) * 100.0;
    sp.size = Size::Percent(factor);
    let report = export::run(gpu, &c, &ids, &sp, &mut |_, _| ControlFlow::Continue(())).unwrap();
    assert!(report.exported.is_empty());
    match &report.failed[0].1 {
        e @ CatalogError::TooLarge { limit, .. } => {
            assert!(*limit > 0);
            assert!(e.to_string().contains("smaller size"), "{e}");
        }
        e => panic!("{e}"),
    }
    assert_eq!(std::fs::read_dir(out.path()).unwrap().count(), 0);
}

#[test]
fn overwrite_replaces_through_a_temp_file_and_leaves_nothing_else() {
    let Some(gpu) = try_shared_gpu() else { return };
    let src = tempfile::tempdir().unwrap();
    png(&src.path().join("a.png"), 32, 32);
    let (c, ids) = catalogue(src.path());
    let out = tempfile::tempdir().unwrap();
    std::fs::write(out.path().join("a.png"), b"old").unwrap();
    let mut sp = spec(Format::Png, out.path());
    sp.overwrite = true;
    let report = export::run(gpu, &c, &ids, &sp, &mut |_, _| ControlFlow::Continue(())).unwrap();
    assert_eq!(report.exported.len(), 1);
    assert_eq!(
        image::image_dimensions(out.path().join("a.png")).unwrap(),
        (32, 32)
    );
    let names: Vec<_> = std::fs::read_dir(out.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["a.png"]);
}

/// A device lost mid-run stops the export at once: the photos after it are never tried (each
/// would only fail the same way), and the report says how far it got.
#[test]
fn a_lost_device_stops_the_run_and_reports_how_far_it_got() {
    // A device of its own: destroying the shared one would break the other tests.
    let Ok(gpu) = lf_pipeline::Gpu::new() else {
        return;
    };
    let src = tempfile::tempdir().unwrap();
    for name in ["a.png", "b.png", "c.png"] {
        png(&src.path().join(name), 32, 32);
    }
    let (c, ids) = catalogue(src.path());
    let out = tempfile::tempdir().unwrap();
    let device = gpu.clone();
    let mut calls = Vec::new();
    let result = export::run(
        gpu.clone(),
        &c,
        &ids,
        &spec(Format::Png, out.path()),
        &mut |done, _| {
            calls.push(done);
            if done == 1 {
                // The driver resets after the first photo; the progress callback never
                // asks to stop, so the library must stop by itself.
                device
                    .device
                    .poll(wgpu::PollType::Wait {
                        submission_index: None,
                        timeout: Some(std::time::Duration::from_secs(30)),
                    })
                    .unwrap();
                device.device.destroy();
                let _ = device.device.poll(wgpu::PollType::Poll);
            }
            ControlFlow::Continue(())
        },
    );
    assert!(gpu.is_lost());
    // The run reports what it wrote before the loss, so the app can say how far it got.
    let report = result.expect("a lost device ends the run with its partial report");
    assert!(report.device_lost);
    assert!(!report.cancelled, "the callback never asked to stop");
    assert_eq!(report.exported.len(), 1);
    assert!(
        report.failed.is_empty(),
        "the lost photo is not a photo failure"
    );
    assert!(
        CatalogError::DeviceLost
            .to_string()
            .contains("graphics device was reset")
    );
    assert_eq!(calls, [0, 1], "no photo is tried after the loss");
    let written: Vec<_> = std::fs::read_dir(out.path()).unwrap().collect();
    assert_eq!(written.len(), 1, "only the first photo was written");
    let job = export::last_job(&c).unwrap().unwrap();
    assert_eq!((job.status.as_str(), job.done), ("failed", 1));
    assert!(job.error.unwrap().contains("graphics device"));
}

fn jpeg_plain(path: &Path) {
    image::RgbImage::from_fn(40, 30, |x, y| Rgb([(x * 5) as u8, (y * 7) as u8, 50]))
        .save_with_format(path, image::ImageFormat::Jpeg)
        .unwrap();
}

/// The desktop never rewrites a source file: an export whose output would land on a
/// catalogued photo is refused before anything is written, even with overwrite on, and
/// whatever the case of the extension.
#[test]
fn exporting_onto_a_source_photo_is_refused_even_when_replacing() {
    let Some(gpu) = try_shared_gpu() else { return };
    let src = tempfile::tempdir().unwrap();
    jpeg_plain(&src.path().join("IMG_1.JPG"));
    png(&src.path().join("other.png"), 24, 16);
    let (c, ids) = catalogue(src.path());
    let jpg = ids
        .iter()
        .copied()
        .find(|id| c.photo(*id).unwrap().unwrap().path.ends_with("IMG_1.JPG"))
        .unwrap();
    let before = std::fs::read(src.path().join("IMG_1.JPG")).unwrap();
    for overwrite in [false, true] {
        let mut sp = spec(Format::Jpeg { quality: 90 }, src.path());
        sp.overwrite = overwrite;
        let e = export::run(gpu.clone(), &c, &[jpg], &sp, &mut |_, _| {
            ControlFlow::Continue(())
        })
        .unwrap_err();
        match &e {
            CatalogError::WouldOverwriteOriginal(path) => {
                assert!(
                    path.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .eq_ignore_ascii_case("IMG_1.jpg")
                );
            }
            other => {
                panic!("overwrite {overwrite}: expected WouldOverwriteOriginal, got {other:?}")
            }
        }
        assert_eq!(
            e.to_string(),
            "IMG_1.jpg is one of your photos. Choose another folder or file name pattern."
        );
        assert_eq!(std::fs::read(src.path().join("IMG_1.JPG")).unwrap(), before);
    }
    let job = export::last_job(&c).unwrap().unwrap();
    assert_eq!(job.status, "failed");
}

/// Every catalogued photo is protected, not only the ones being exported.
#[test]
fn a_catalogued_photo_outside_the_selection_is_protected_too() {
    let Some(gpu) = try_shared_gpu() else { return };
    let src = tempfile::tempdir().unwrap();
    png(&src.path().join("a.png"), 24, 16);
    png(&src.path().join("b.png"), 30, 20);
    let (c, ids) = catalogue(src.path());
    let a = ids
        .iter()
        .copied()
        .find(|id| c.photo(*id).unwrap().unwrap().path.ends_with("a.png"))
        .unwrap();
    let before = std::fs::read(src.path().join("b.png")).unwrap();
    let mut sp = spec(Format::Png, src.path());
    sp.rename = Pattern("b".into());
    sp.overwrite = true;
    let e = export::run(gpu, &c, &[a], &sp, &mut |_, _| ControlFlow::Continue(())).unwrap_err();
    assert!(
        matches!(&e, CatalogError::WouldOverwriteOriginal(p) if p.ends_with("b.png")),
        "{e:?}"
    );
    assert_eq!(std::fs::read(src.path().join("b.png")).unwrap(), before);
}

/// The ICC profile and EXIF a written file carries, as the image crate's decoders read them.
fn tags(path: &Path) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    use image::ImageDecoder;
    let reader = ImageReader::open(path)
        .unwrap()
        .with_guessed_format()
        .unwrap();
    let mut decoder = reader.into_decoder().unwrap();
    (
        decoder.icc_profile().unwrap(),
        decoder.exif_metadata().unwrap(),
    )
}

#[test]
fn every_format_carries_the_srgb_profile_and_png_and_webp_keep_the_exif() {
    let Some(gpu) = try_shared_gpu() else { return };
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("j.jpg");
    jpeg_with_exif(&file);
    let (c, ids) = catalogue(src.path());
    let icc = lf_catalog::finish::srgb_icc();
    for format in [
        Format::Jpeg { quality: 85 },
        Format::Png,
        Format::WebP,
        Format::Tiff16,
    ] {
        let out = tempfile::tempdir().unwrap();
        let mut sp = spec(format, out.path());
        sp.metadata = Metadata::All;
        let done = export::run(gpu.clone(), &c, &ids, &sp, &mut |_, _| {
            ControlFlow::Continue(())
        })
        .unwrap();
        assert!(done.failed.is_empty(), "{format:?}: {:?}", done.failed);
        let path = &done.exported[0].path;
        let (profile, exif) = tags(path);
        if format == Format::Tiff16 {
            // image 0.25's TIFF decoder does not hand the profile back: look for it in the file.
            let bytes = std::fs::read(path).unwrap();
            assert!(
                bytes.windows(icc.len()).any(|w| w == icc.as_slice()),
                "the TIFF carries the sRGB profile"
            );
        } else {
            assert_eq!(
                profile.as_ref(),
                Some(&icc),
                "{format:?} carries the sRGB profile"
            );
        }
        match format {
            Format::Png | Format::WebP => {
                let exif = exif.unwrap_or_else(|| panic!("{format:?} keeps the EXIF"));
                // The TIFF part of the source's APP1: Make = "Cam".
                assert!(exif.starts_with(b"MM\0\x2a"), "{format:?}");
                assert!(exif.windows(4).any(|w| w == b"Cam\0"), "{format:?}");
            }
            Format::Jpeg { .. } => {
                let bytes = std::fs::read(path).unwrap();
                assert_eq!(read_summary(&bytes).make.as_deref(), Some("Cam"));
                assert_eq!(bytes.windows(6).filter(|w| *w == b"Exif\0\0").count(), 1);
            }
            // Not in this loop: `lossy_webp_avif_png_and_tiff_carry_what_they_can` covers them.
            Format::Tiff16 | Format::WebPLossy { .. } | Format::Avif { .. } => {}
        }
        // The pixels decode, at the photo's size.
        let img = image::open(path).unwrap();
        assert_eq!((img.width(), img.height()), (64, 48), "{format:?}");
    }
}

#[test]
fn a_watermark_and_sharpening_change_only_what_they_should() {
    use lf_catalog::finish::{Corner, Sharpen, Stamp, Watermark};
    let Some(gpu) = try_shared_gpu() else { return };
    let src = tempfile::tempdir().unwrap();
    // A flat grey photo: sharpening leaves it alone, so any change is the watermark's.
    RgbaImage::from_pixel(400, 300, image::Rgba([110, 110, 110, 255]))
        .save(src.path().join("g.png"))
        .unwrap();
    let (c, ids) = catalogue(src.path());
    let export_with = |sharpen, watermark| {
        let out = tempfile::tempdir().unwrap();
        let mut sp = spec(Format::Png, out.path());
        sp.sharpen = sharpen;
        sp.watermark = watermark;
        let done = export::run(gpu.clone(), &c, &ids, &sp, &mut |_, _| {
            ControlFlow::Continue(())
        })
        .unwrap();
        assert!(done.failed.is_empty(), "{:?}", done.failed);
        let img = image::open(&done.exported[0].path).unwrap().to_rgb8();
        (out, img)
    };
    let (_a, plain) = export_with(Sharpen::Off, None);
    let (_b, sharpened) = export_with(Sharpen::High, None);
    assert_eq!(
        plain, sharpened,
        "a flat photo is not changed by sharpening"
    );
    let (_c, marked) = export_with(
        Sharpen::Standard,
        Some(Watermark {
            stamp: Stamp::Text("© Studio".into()),
            width: 0.4,
            opacity: 0.8,
            corner: Corner::TopLeft,
        }),
    );
    let differs = |x0: u32, x1: u32, y0: u32, y1: u32| {
        (y0..y1).any(|y| (x0..x1).any(|x| marked.get_pixel(x, y) != plain.get_pixel(x, y)))
    };
    assert!(differs(0, 200, 0, 100), "the top left carries the mark");
    assert!(!differs(220, 400, 0, 300), "the right is untouched");
    assert!(!differs(0, 400, 150, 300), "the bottom is untouched");
}

#[test]
fn a_missing_watermark_logo_stops_the_export_before_anything_is_written() {
    use lf_catalog::finish::{Corner, Stamp, Watermark};
    let Some(gpu) = try_shared_gpu() else { return };
    let src = tempfile::tempdir().unwrap();
    png(&src.path().join("a.png"), 40, 30);
    let (c, ids) = catalogue(src.path());
    let out = tempfile::tempdir().unwrap();
    let mut sp = spec(Format::Png, out.path());
    sp.watermark = Some(Watermark {
        stamp: Stamp::Image(src.path().join("no-such-logo.png")),
        width: 0.2,
        opacity: 0.5,
        corner: Corner::BottomRight,
    });
    let e = export::run(gpu, &c, &ids, &sp, &mut |_, _| ControlFlow::Continue(()))
        .unwrap_err()
        .to_string();
    assert!(e.contains("could not be read"), "{e}");
    assert_eq!(std::fs::read_dir(out.path()).unwrap().count(), 0);
}

#[test]
fn a_jpeg_carries_the_catalogues_metadata_as_xmp_and_exif() {
    use lf_catalog::meta::Field;
    let Some(gpu) = try_shared_gpu() else { return };
    let src = tempfile::tempdir().unwrap();
    png(&src.path().join("a.png"), 40, 30);
    let (c, ids) = catalogue(src.path());
    c.set_meta(&ids, Field::Title, "Mehndi night").unwrap();
    c.set_meta(&ids, Field::Creator, "Asha").unwrap();
    c.add_keywords(&ids, &["wedding".to_string(), "Jaipur".to_string()])
        .unwrap();
    let out = tempfile::tempdir().unwrap();
    let mut sp = spec(Format::Jpeg { quality: 90 }, out.path());
    sp.metadata = Metadata::All;
    sp.rename = lf_catalog::export::Pattern("{title}-{seq}".into());
    let done = export::run(gpu, &c, &ids, &sp, &mut |_, _| ControlFlow::Continue(())).unwrap();
    // `{title}` names the file after the catalogue title.
    assert_eq!(
        done.exported[0].path.file_name().unwrap(),
        "Mehndi night-1.jpg"
    );
    let bytes = std::fs::read(&done.exported[0].path).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("http://ns.adobe.com/xap/1.0/"));
    assert!(text.contains("Mehndi night"));
    assert!(text.contains("<rdf:li>Jaipur</rdf:li><rdf:li>wedding</rdf:li>"));
    // The EXIF's Artist is the catalogue's creator.
    assert_eq!(read_summary(&bytes).artist.as_deref(), Some("Asha"));
    // The file still decodes.
    image::load_from_memory(&bytes).unwrap();

    // Copyright only: the creator, no title or keywords.
    let out = tempfile::tempdir().unwrap();
    let mut sp = spec(Format::Jpeg { quality: 90 }, out.path());
    sp.metadata = Metadata::CopyrightOnly;
    let done = export::run(try_shared_gpu().unwrap(), &c, &ids, &sp, &mut |_, _| {
        ControlFlow::Continue(())
    })
    .unwrap();
    let text =
        String::from_utf8_lossy(&std::fs::read(&done.exported[0].path).unwrap()).into_owned();
    assert!(text.contains("Asha") && !text.contains("Mehndi") && !text.contains("Jaipur"));
}

#[test]
fn lossy_webp_avif_png_and_tiff_carry_what_they_can() {
    use lf_catalog::meta::Field;
    let Some(gpu) = try_shared_gpu() else { return };
    let src = tempfile::tempdir().unwrap();
    png(&src.path().join("a.png"), 64, 48);
    let (c, ids) = catalogue(src.path());
    c.set_meta(&ids, Field::Title, "Haldi").unwrap();
    c.set_meta(&ids, Field::Creator, "Asha").unwrap();
    let run = |format: Format| {
        let out = tempfile::tempdir().unwrap();
        let mut sp = spec(format, out.path());
        sp.metadata = Metadata::All;
        let done = export::run(try_shared_gpu().unwrap(), &c, &ids, &sp, &mut |_, _| {
            ControlFlow::Continue(())
        })
        .unwrap();
        (std::fs::read(&done.exported[0].path).unwrap(), out)
    };
    let _ = gpu;
    // Lossy WebP: an extended file with the profile, EXIF and XMP; it decodes.
    let (webp, _keep) = run(Format::WebPLossy { quality: 80 });
    let text = String::from_utf8_lossy(&webp);
    assert!(text.contains("VP8X") && text.contains("ICCP") && text.contains("Haldi"));
    assert_eq!(image::load_from_memory(&webp).unwrap().width(), 64);
    // Lossless WebP carries the same.
    let (webp, _keep) = run(Format::WebP);
    assert!(String::from_utf8_lossy(&webp).contains("Haldi"));
    // PNG: XMP in iTXt.
    let (png_bytes, _keep) = run(Format::Png);
    assert!(String::from_utf8_lossy(&png_bytes).contains("XML:com.adobe.xmp"));
    // TIFF: XMP and the artist tag.
    let (tif, _keep) = run(Format::Tiff16);
    let text = String::from_utf8_lossy(&tif);
    assert!(text.contains("Haldi") && text.contains("Asha"));
    assert!(matches!(
        image::load_from_memory(&tif).unwrap(),
        image::DynamicImage::ImageRgb16(_)
    ));
    // AVIF: an AVIF file.
    let (avif, _keep) = run(Format::Avif { quality: 70 });
    assert_eq!(&avif[4..12], b"ftypavif");
}

#[test]
fn an_export_in_adobe_rgb_is_converted_and_carries_that_profile() {
    let Some(gpu) = try_shared_gpu() else { return };
    let src = tempfile::tempdir().unwrap();
    png(&src.path().join("a.png"), 32, 24);
    let (c, ids) = catalogue(src.path());
    let out = tempfile::tempdir().unwrap();
    let mut sp = spec(Format::Png, out.path());
    sp.space = lf_catalog::colour::OutputSpace::AdobeRgb;
    let done = export::run(gpu, &c, &ids, &sp, &mut |_, _| ControlFlow::Continue(())).unwrap();
    let bytes = std::fs::read(&done.exported[0].path).unwrap();
    let mut decoder = image::codecs::png::PngDecoder::new(std::io::Cursor::new(&bytes)).unwrap();
    let profile = image::ImageDecoder::icc_profile(&mut decoder)
        .unwrap()
        .unwrap();
    assert_ne!(
        profile,
        lf_catalog::finish::srgb_icc(),
        "the Adobe RGB profile is embedded"
    );
    // AVIF stays sRGB only.
    let mut avif = spec(Format::Avif { quality: 60 }, out.path());
    avif.space = lf_catalog::colour::OutputSpace::DisplayP3;
    let report = export::run(try_shared_gpu().unwrap(), &c, &ids, &avif, &mut |_, _| {
        ControlFlow::Continue(())
    })
    .unwrap();
    assert_eq!(report.exported.len(), 0);
    assert!(report.failed[0].1.to_string().contains("sRGB only"));
}
