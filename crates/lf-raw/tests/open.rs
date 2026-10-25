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
    assert_eq!(&kept[26..28], &[0, 1], "orientation patched to 1");
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
        Err(RawError::Unsupported { reason, .. }) => {
            assert_eq!(reason, "HEIC files aren’t supported yet.")
        }
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
