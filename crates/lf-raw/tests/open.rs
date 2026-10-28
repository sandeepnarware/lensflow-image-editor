mod support {
    pub mod synthetic_dng;
}

use image::{ImageFormat, Rgb, Rgb32FImage, RgbImage, Rgba, RgbaImage};
use lf_raw::RawError;
use lf_raw::demosaic::Quality;
use lf_raw::open::{Kind, Picture, open, sniff};
use lf_settings::DevelopSettings;
use std::path::PathBuf;
use support::synthetic_dng;

fn tmp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("lf-raw-open-{}-{name}", std::process::id()))
}

fn open_default(p: &std::path::Path) -> Result<lf_raw::open::Opened, RawError> {
    open(p, Quality::Fast, &DevelopSettings::default(), 4096)
}

fn eight(o: &lf_raw::open::Opened) -> &RgbaImage {
    match &o.picture {
        Picture::EightBit(i) => i,
        Picture::Float(_) => panic!("expected an 8-bit picture"),
    }
}

fn px(o: &lf_raw::open::Opened, x: u32, y: u32) -> [f32; 3] {
    let p = eight(o).get_pixel(x, y).0;
    [0, 1, 2].map(|c| p[c] as f32 / 255.0)
}

/// A 16x8 picture of four flat 8x8-aligned blocks: red top-left, green bottom-left,
/// blue top-right, white bottom-right (flat blocks survive JPEG compression).
fn quadrants() -> RgbImage {
    RgbImage::from_fn(16, 8, |x, y| match (x < 8, y < 4) {
        (true, true) => Rgb([255, 0, 0]),
        (true, false) => Rgb([0, 255, 0]),
        (false, true) => Rgb([0, 0, 255]),
        (false, false) => Rgb([255, 255, 255]),
    })
}

/// Insert a hand-built APP1 Exif segment carrying only Orientation = `o` after SOI.
fn with_orientation(jpeg: &[u8], o: u16) -> Vec<u8> {
    let mut seg = vec![0xff, 0xe1, 0, 34];
    seg.extend_from_slice(b"Exif\0\0");
    seg.extend_from_slice(b"MM\0*\0\0\0\x08"); // big-endian TIFF header, IFD0 at 8
    seg.extend_from_slice(&[0, 1]); // one entry
    seg.extend_from_slice(&[0x01, 0x12, 0, 3, 0, 0, 0, 1]); // Orientation, SHORT, 1
    seg.extend_from_slice(&o.to_be_bytes());
    seg.extend_from_slice(&[0, 0]);
    seg.extend_from_slice(&[0, 0, 0, 0]); // no IFD1
    let mut out = jpeg[..2].to_vec();
    out.extend_from_slice(&seg);
    out.extend_from_slice(&jpeg[2..]);
    out
}

fn jpeg_bytes(img: &RgbImage) -> Vec<u8> {
    let mut v = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut v), ImageFormat::Jpeg)
        .unwrap();
    v
}

#[test]
fn jpeg_with_orientation_6_is_turned_upright() {
    let path = tmp("o6.jpg");
    std::fs::write(&path, with_orientation(&jpeg_bytes(&quadrants()), 6)).unwrap();
    let o = open_default(&path).unwrap();
    assert_eq!(o.kind, Kind::Jpeg);
    assert_eq!(o.exif.orientation, 6);
    // 16x8 turned 90 degrees clockwise is 8x16.
    assert_eq!(eight(&o).dimensions(), (8, 16));
    assert_eq!((o.full_width, o.full_height, o.reduced), (8, 16, false));
    // Rotate90: the original's bottom-left (green) becomes the top-left; top-left
    // (red) becomes top-right.
    let tl = px(&o, 1, 1);
    assert!(tl[1] > 0.9 && tl[0] < 0.1 && tl[2] < 0.1, "{tl:?}");
    let tr = px(&o, 6, 1);
    assert!(tr[0] > 0.9 && tr[1] < 0.1 && tr[2] < 0.1, "{tr:?}");
    // The kept Exif segment says upright and has the marker.
    let kept = o.jpeg_exif_bytes.expect("exif bytes");
    assert_eq!(&kept[..2], &[0xff, 0xe1]);
    assert_eq!(&kept[4..10], b"Exif\0\0");
    assert_eq!(&kept[28..30], &[0, 1], "orientation patched to 1");
    let _ = std::fs::remove_file(path);
}

#[test]
fn jpeg_without_exif_is_upright_with_no_exif_bytes() {
    let path = tmp("plain.jpg");
    std::fs::write(&path, jpeg_bytes(&quadrants())).unwrap();
    let o = open_default(&path).unwrap();
    assert_eq!(eight(&o).dimensions(), (16, 8));
    assert_eq!(o.exif.orientation, 1);
    assert!(o.jpeg_exif_bytes.is_none());
    let _ = std::fs::remove_file(path);
}

#[test]
fn png_opens_with_exact_8_bit_values() {
    let path = tmp("a.png");
    let img = RgbaImage::from_fn(5, 3, |x, y| Rgba([(x * 50) as u8, (y * 100) as u8, 7, 255]));
    img.save_with_format(&path, ImageFormat::Png).unwrap();
    let o = open_default(&path).unwrap();
    assert_eq!(o.kind, Kind::Png);
    assert_eq!(eight(&o), &img);
    assert!(o.jpeg_exif_bytes.is_none());
    let _ = std::fs::remove_file(path);
}

#[test]
fn plain_tiff_opens_as_tiff() {
    let path = tmp("a.tif");
    quadrants()
        .save_with_format(&path, ImageFormat::Tiff)
        .unwrap();
    assert_eq!(sniff(&path).unwrap(), Kind::Tiff);
    let o = open_default(&path).unwrap();
    assert_eq!(o.kind, Kind::Tiff);
    assert_eq!(eight(&o).dimensions(), (16, 8));
    let _ = std::fs::remove_file(path);
}

#[test]
fn big_png_is_box_reduced_to_max_edge() {
    let path = tmp("big.png");
    RgbaImage::from_pixel(100, 40, Rgba([10, 20, 30, 255]))
        .save_with_format(&path, ImageFormat::Png)
        .unwrap();
    let o = open(&path, Quality::Fast, &DevelopSettings::default(), 50).unwrap();
    assert_eq!(eight(&o).dimensions(), (50, 20));
    assert_eq!((o.full_width, o.full_height, o.reduced), (100, 40, true));
    assert_eq!(eight(&o).get_pixel(3, 3).0, [10, 20, 30, 255]);
    let _ = std::fs::remove_file(path);
}

#[test]
fn webp_is_sniffed_by_magic_not_extension() {
    let path = tmp("webp.bin");
    let mut bytes = b"RIFF\x04\0\0\0WEBP".to_vec();
    bytes.extend_from_slice(b"VP8 ");
    std::fs::write(&path, &bytes).unwrap();
    assert_eq!(sniff(&path).unwrap(), Kind::WebP);
    let _ = std::fs::remove_file(path);
}

#[test]
fn text_file_is_not_an_image() {
    let path = tmp("notes.txt");
    std::fs::write(&path, "hello, not a photo").unwrap();
    assert!(matches!(
        open_default(&path),
        Err(RawError::NotAnImage { .. })
    ));
    let _ = std::fs::remove_file(path);
}

#[test]
fn heic_is_refused_with_a_clear_message() {
    let path = tmp("a.heic");
    std::fs::write(&path, b"\0\0\0\x18ftypheic\0\0\0\0mif1heic").unwrap();
    assert_eq!(sniff(&path).unwrap(), Kind::Heic);
    match open_default(&path) {
        // A broken HEIC: the system decoder's own reason (or what to install).
        Err(RawError::Unsupported { reason, .. }) => assert!(!reason.is_empty()),
        other => panic!("{other:?}"),
    }
    let _ = std::fs::remove_file(path);
}

#[test]
fn cr3_brand_is_raw_and_missing_file_is_io() {
    let path = tmp("a.cr3");
    std::fs::write(&path, b"\0\0\0\x18ftypcrx \0\0\0\x01").unwrap();
    assert_eq!(sniff(&path).unwrap(), Kind::Raw);
    let _ = std::fs::remove_file(&path);
    assert!(matches!(sniff(&path), Err(RawError::Io { .. })));
}

#[test]
fn synthetic_dng_is_raw_and_opens() {
    let path = tmp("a.dng");
    let img = Rgb32FImage::from_fn(64, 48, |x, y| {
        Rgb([0.2 + x as f32 / 128.0, 0.3 + y as f32 / 128.0, 0.4])
    });
    synthetic_dng::write(
        &path,
        &img,
        synthetic_dng::RGGB,
        64,
        4095,
        [2.0, 1.0, 1.5],
        [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
    );
    assert_eq!(sniff(&path).unwrap(), Kind::Raw);
    let o = open_default(&path).unwrap();
    assert_eq!(o.kind, Kind::Raw);
    assert_eq!(o.exif.orientation, 1);
    assert!(o.jpeg_exif_bytes.is_none());
    let Picture::Float(d) = &o.picture else {
        panic!("RAW is a float picture")
    };
    assert!(d.frame.width > 0);
    let _ = std::fs::remove_file(path);
}

/// A segment with `order` byte order, Orientation 6 and a non-zero IFD1 link.
fn exif_segment(little: bool) -> Vec<u8> {
    let u16b = |v: u16| {
        if little {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        }
    };
    let u32b = |v: u32| {
        if little {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        }
    };
    let mut seg = vec![0xff, 0xe1, 0, 34];
    seg.extend_from_slice(b"Exif\0\0");
    seg.extend_from_slice(if little { b"II*\0" } else { b"MM\0*" });
    seg.extend_from_slice(&u32b(8));
    seg.extend_from_slice(&u16b(1));
    seg.extend_from_slice(&u16b(0x0112));
    seg.extend_from_slice(&u16b(3));
    seg.extend_from_slice(&u32b(1));
    seg.extend_from_slice(&u16b(6));
    seg.extend_from_slice(&[0, 0]);
    seg.extend_from_slice(&u32b(0x1234)); // IFD1 link
    seg
}

#[test]
fn extracted_exif_is_patched_in_both_byte_orders() {
    for little in [false, true] {
        let seg = exif_segment(little);
        let mut jpeg = vec![0xff, 0xd8];
        jpeg.extend_from_slice(&seg);
        jpeg.extend_from_slice(&[0xff, 0xd9]);
        let kept = lf_raw::exif::extract_jpeg_exif(&jpeg).expect("segment");
        assert_eq!(kept.len(), seg.len());
        let one = if little { [1, 0] } else { [0, 1] };
        assert_eq!(&kept[28..30], &one, "orientation is 1 (little={little})");
        assert_eq!(
            &kept[32..36],
            &[0, 0, 0, 0],
            "IFD1 link zeroed (little={little})"
        );
        // Untouched parts stay as they were.
        assert_eq!(&kept[..28], &seg[..28]);
    }
}

#[test]
fn orient_rgba8_matches_hand_computed_tables_and_develop_rgb8() {
    use lf_raw::Orientation;
    use lf_raw::develop::{develop_rgb8, orient_rgba8};
    // 3x2, pixel id = 10 * y + x + 1 in the red channel:
    //   1 2 3
    //  11 12 13
    let img = RgbaImage::from_fn(3, 2, |x, y| Rgba([(10 * y + x + 1) as u8, 0, 0, 255]));
    let expected: [(u8, (u32, u32), [u8; 6]); 8] = [
        (1, (3, 2), [1, 2, 3, 11, 12, 13]),
        (2, (3, 2), [3, 2, 1, 13, 12, 11]),
        (3, (3, 2), [13, 12, 11, 3, 2, 1]),
        (4, (3, 2), [11, 12, 13, 1, 2, 3]),
        (5, (2, 3), [1, 11, 2, 12, 3, 13]),
        (6, (2, 3), [11, 1, 12, 2, 13, 3]),
        (7, (2, 3), [13, 3, 12, 2, 11, 1]),
        (8, (2, 3), [3, 13, 2, 12, 1, 11]),
    ];
    for (o, dims, reds) in expected {
        let orientation = Orientation::from_exif(o as u16);
        let out = orient_rgba8(img.clone(), orientation);
        assert_eq!(out.dimensions(), dims, "orientation {o}");
        let got: Vec<u8> = out.pixels().map(|p| p.0[0]).collect();
        assert_eq!(got, reds, "orientation {o}");
        let f = develop_rgb8(&img, orientation).frame;
        assert_eq!((f.width, f.height), dims);
        for (i, p) in out.pixels().enumerate() {
            for c in 0..4 {
                assert_eq!(
                    f.data[i * 4 + c],
                    half::f16::from_f32(p.0[c] as f32 / 255.0)
                );
            }
        }
    }
}

/// One IFD entry for [`hand_tiff`]: tag, type, count and the value bytes (stored
/// out of line when longer than 4).
type Entry = (u16, u16, u32, Vec<u8>);

fn ascii(tag: u16, s: &str) -> Entry {
    let mut v = s.as_bytes().to_vec();
    v.push(0);
    (tag, 2, v.len() as u32, v)
}

fn short(tag: u16, vals: &[u16]) -> Entry {
    let v = vals.iter().flat_map(|x| x.to_le_bytes()).collect();
    (tag, 3, vals.len() as u32, v)
}

fn long(tag: u16, val: u32) -> Entry {
    (tag, 4, 1, val.to_le_bytes().to_vec())
}

/// A one-strip 4x4 8-bit TIFF, little-endian: RGB when `rgb`, else greyscale. Entries
/// in `extra` replace the base ones with the same tag.
fn hand_tiff(rgb: bool, extra: &[Entry]) -> Vec<u8> {
    let spp: u16 = if rgb { 3 } else { 1 };
    let mut entries: Vec<Entry> = vec![
        long(0x0100, 4),
        long(0x0101, 4),
        short(0x0102, &vec![8; spp as usize]),
        short(0x0103, &[1]),
        short(0x0106, &[if rgb { 2 } else { 1 }]),
        long(0x0111, 0), // patched below
        short(0x0115, &[spp]),
        long(0x0116, 4),
        long(0x0117, 16 * spp as u32),
    ];
    entries.retain(|e| !extra.iter().any(|x| x.0 == e.0));
    entries.extend_from_slice(extra);
    entries.sort_by_key(|e| e.0);
    let ifd_len = 2 + entries.len() as u32 * 12 + 4;
    let mut data_at = 8 + ifd_len;
    let mut out_of_line = Vec::new();
    let mut ifd = (entries.len() as u16).to_le_bytes().to_vec();
    let mut strip_entry = 0;
    for (tag, ty, count, val) in &entries {
        ifd.extend_from_slice(&tag.to_le_bytes());
        ifd.extend_from_slice(&ty.to_le_bytes());
        ifd.extend_from_slice(&count.to_le_bytes());
        if *tag == 0x0111 {
            strip_entry = ifd.len();
        }
        if val.len() <= 4 {
            let mut v = val.clone();
            v.resize(4, 0);
            ifd.extend_from_slice(&v);
        } else {
            ifd.extend_from_slice(&data_at.to_le_bytes());
            out_of_line.extend_from_slice(val);
            if val.len() % 2 == 1 {
                out_of_line.push(0);
            }
            data_at = 8 + ifd_len + out_of_line.len() as u32;
        }
    }
    ifd.extend_from_slice(&[0, 0, 0, 0]);
    ifd[strip_entry..strip_entry + 4].copy_from_slice(&data_at.to_le_bytes());
    let mut out = b"II*\0".to_vec();
    out.extend_from_slice(&8u32.to_le_bytes());
    out.extend_from_slice(&ifd);
    out.extend_from_slice(&out_of_line);
    out.extend_from_slice(&vec![7u8; 16 * spp as usize]);
    out
}

#[test]
fn a_dng_tagged_tiff_is_never_opened_as_a_preview() {
    let path = tmp("fake.dng");
    // DNGVersion 1.4.0.0 on an otherwise bare 4x4 image (rawler takes it as a DNG and
    // then finds no CFA layout).
    std::fs::write(&path, hand_tiff(false, &[(0xC612, 1, 4, vec![1, 4, 0, 0])])).unwrap();
    assert_eq!(sniff(&path).unwrap(), Kind::Raw);
    let r = open_default(&path);
    assert!(
        matches!(
            r,
            Err(RawError::Unsupported { .. } | RawError::Corrupt { .. })
        ),
        "{r:?}"
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_cfa_tiff_rawler_rejects_is_unsupported_naming_the_camera() {
    let path = tmp("acme.nef");
    // Make "Acme", Model "X1" and CFA sensor data: a RAW from a maker rawler lacks.
    let tiff = hand_tiff(
        false,
        &[
            ascii(0x010F, "Acme"),
            ascii(0x0110, "X1"),
            short(0x0106, &[32803]),
        ],
    );
    std::fs::write(&path, tiff).unwrap();
    assert_eq!(sniff(&path).unwrap(), Kind::Raw);
    match open_default(&path) {
        Err(RawError::Unsupported { reason, .. }) => {
            assert_eq!(reason, "Acme X1 RAW files aren’t supported yet.")
        }
        other => panic!("{other:?}"),
    }
    let _ = std::fs::remove_file(path);
}

#[test]
fn camera_maker_tiff_exports_open_as_pictures() {
    // Lightroom, Photoshop and DPP keep the camera's Make and Model in exported TIFFs;
    // rawler would pick its CR2 (Canon) or 3FR (Hasselblad) decoder by Make alone.
    for (make, model) in [
        ("Canon", "Canon EOS R5"),
        ("Canon", "Canon EOS 5D Mark IV"),
        ("Hasselblad", "X2D 100C"),
        ("NIKON CORPORATION", "NIKON Z 8"),
        ("SONY", "ILCE-7RM5"),
    ] {
        let path = tmp(&format!("export-{}.tif", make.replace(' ', "_")));
        let tiff = hand_tiff(
            true,
            &[
                ascii(0x010F, make),
                ascii(0x0110, model),
                ascii(0x0131, "Adobe Photoshop Lightroom Classic 13.0 (Windows)"),
                long(0x014A, 0),
            ],
        );
        std::fs::write(&path, tiff).unwrap();
        assert_eq!(sniff(&path).unwrap(), Kind::Tiff, "{make}");
        let o = open_default(&path).unwrap_or_else(|e| panic!("{make}: {e}"));
        assert_eq!(o.kind, Kind::Tiff);
        assert_eq!(eight(&o).dimensions(), (4, 4));
        assert_eq!(eight(&o).get_pixel(1, 1).0, [7, 7, 7, 255]);
        assert_eq!(o.exif.make.as_deref(), Some(make));
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn a_bare_hand_built_tiff_still_opens_as_a_picture() {
    let path = tmp("bare.tif");
    std::fs::write(&path, hand_tiff(false, &[])).unwrap();
    let o = open_default(&path).unwrap();
    assert_eq!(o.kind, Kind::Tiff);
    assert_eq!(eight(&o).dimensions(), (4, 4));
    let _ = std::fs::remove_file(path);
}

#[test]
fn not_an_image_message_is_plain() {
    let path = tmp("notes2.txt");
    std::fs::write(&path, "hello").unwrap();
    let e = open_default(&path).unwrap_err();
    assert_eq!(
        e.to_string(),
        format!("{} is not a photo this app can open.", path.display())
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn avif_and_heif_brands_get_their_own_messages() {
    for (brand, msg) in [
        (b"avif", "AVIF files aren’t supported yet."),
        (b"avis", "AVIF files aren’t supported yet."),
        // HEIF brands go to the system decoder, which refuses these empty files.
        (b"heif", ""),
        (b"hevm", ""),
        (b"hevs", ""),
    ] {
        let path = tmp(&format!("x-{}.bin", String::from_utf8_lossy(brand)));
        let mut b = b"\0\0\0\x18ftyp".to_vec();
        b.extend_from_slice(brand);
        b.extend_from_slice(&[0; 8]);
        std::fs::write(&path, b).unwrap();
        match open_default(&path) {
            Err(RawError::Unsupported { reason, .. }) if msg.is_empty() => {
                assert!(!reason.is_empty() && !reason.contains("AVIF"));
            }
            Err(RawError::Unsupported { reason, .. }) => assert_eq!(reason, msg),
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn canon_crw_magic_is_raw() {
    let path = tmp("a.crw");
    let mut b = b"II\x1a\0\0\0HEAPCCDR".to_vec();
    b.extend_from_slice(&[0; 8]);
    std::fs::write(&path, b).unwrap();
    assert_eq!(sniff(&path).unwrap(), Kind::Raw);
    let _ = std::fs::remove_file(path);
}
