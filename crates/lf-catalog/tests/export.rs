mod support {
    pub mod synthetic_dng;
}

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
    }
}

fn catalogue(dir: &Path) -> (Catalog, Vec<i64>) {
    let c = Catalog::open(dir).unwrap();
    let f = c.add_folder("f").unwrap();
    import::run(&c, f, &import::scan(dir), 200, &mut |_| Ok(())).unwrap();
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
        &mut |d, t| seen.push((d, t)),
    )
    .unwrap();
    let (w, h) = output_size(120, 80, &Geometry::from(&s));
    assert_eq!((done[0].width, done[0].height), (w, h));
    assert_eq!(image::image_dimensions(&done[0].path).unwrap(), (w, h));
    assert_eq!(done[0].path, out.path().join("a.jpg"));
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
    let done = export::run(gpu, &c, &ids, &sp, &mut |_, _| {}).unwrap();
    assert_eq!(image::image_dimensions(&done[0].path).unwrap(), (60, 40));
    assert_eq!((done[0].width, done[0].height), (60, 40));
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
    let done = export::run(gpu, &c, &ids, &sp, &mut |_, _| {}).unwrap();
    let bytes = std::fs::read(&done[0].path).unwrap();
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
        &mut |_, _| {},
    )
    .unwrap();
    assert_eq!(done[0].path.extension().unwrap(), "tif");
    let img = ImageReader::open(&done[0].path).unwrap().decode().unwrap();
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
        let done = export::run(gpu.clone(), &c, &ids, &sp, &mut |_, _| {}).unwrap();
        let s = read_summary(&std::fs::read(&done[0].path).unwrap());
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
    let done = export::run(gpu, &c, &ids, &sp, &mut |_, _| {}).unwrap();
    let names: Vec<String> = done
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
    export::run(gpu.clone(), &c, &ids, &sp, &mut |_, _| {}).unwrap();
    let before = std::fs::read(out.path().join("a.png")).unwrap();
    let err = export::run(gpu.clone(), &c, &ids, &sp, &mut |_, _| {}).unwrap_err();
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
    export::run(gpu, &c, &ids, &again, &mut |_, _| {}).unwrap();
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
        let err = export::run(gpu.clone(), &c, &ids, &sp, &mut |_, _| {}).unwrap_err();
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
        export::run(gpu, &c, &ids, &sp, &mut |_, _| {})
            .unwrap()
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
        &mut |_, _| {},
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
