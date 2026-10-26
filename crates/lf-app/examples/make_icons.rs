//! Makes the installers' icons from the logo:
//! `cargo run -p lf-app --example make_icons -- assets/logo.png packaging/icons`
//! (or `scripts/release/make-icons.sh` / `.ps1`). Writes `16.png` … `512.png`, `app.ico`
//! (Windows: 16–256, PNG entries) and `app.icns` (macOS: PNG entries up to 512, including
//! the @2x sizes). Both containers are written directly (each is a short header and a table
//! of PNG blobs), so no system tools and no extra crates are needed. The output is
//! deterministic: the same logo gives the same bytes on every platform.

use image::imageops::FilterType;
use image::{ImageFormat, RgbaImage};
use std::io::Cursor;
use std::path::{Path, PathBuf};

const PNG_SIZES: [u32; 8] = [16, 24, 32, 48, 64, 128, 256, 512];
const ICO_SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];
/// ICNS element types holding PNG data, with their pixel size (the @2x types hold the
/// doubled size).
const ICNS_TYPES: [(&[u8; 4], u32); 9] = [
    (b"icp4", 16),
    (b"icp5", 32),
    (b"icp6", 64),
    (b"ic07", 128),
    (b"ic08", 256),
    (b"ic09", 512),
    (b"ic11", 32),
    (b"ic12", 64),
    (b"ic13", 256),
];

fn main() {
    let mut args = std::env::args().skip(1);
    let usage = "usage: make_icons <logo.png> <output dir>";
    let logo = PathBuf::from(args.next().expect(usage));
    let out = PathBuf::from(args.next().expect(usage));
    let source = image::open(&logo)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", logo.display()))
        .to_rgba8();
    assert_eq!(
        source.dimensions(),
        (512, 512),
        "the logo must be 512 x 512 px"
    );
    std::fs::create_dir_all(&out).expect("create the output folder");

    for size in PNG_SIZES {
        write(&out.join(format!("{size}.png")), &png(&source, size));
    }
    write(&out.join("app.ico"), &ico(&source));
    write(&out.join("app.icns"), &icns(&source));
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
    println!("{} ({} bytes)", path.display(), bytes.len());
}

/// The logo at `size` px, PNG-encoded.
fn png(source: &RgbaImage, size: u32) -> Vec<u8> {
    let resized = if size == source.width() {
        source.clone()
    } else {
        image::imageops::resize(source, size, size, FilterType::Lanczos3)
    };
    let mut bytes = Vec::new();
    resized
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
        .expect("encode PNG");
    bytes
}

/// ICONDIR, one ICONDIRENTRY per size, then the PNG images (Windows Vista and later read
/// PNG entries; a width or height of 256 is stored as 0).
fn ico(source: &RgbaImage) -> Vec<u8> {
    let images: Vec<(u32, Vec<u8>)> = ICO_SIZES.iter().map(|&s| (s, png(source, s))).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // type: icon
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, data) in &images {
        let dim = if *size >= 256 { 0 } else { *size as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0]); // width, height, palette, reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // colour planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for (_, data) in &images {
        out.extend_from_slice(data);
    }
    out
}

/// `icns` + total length, then elements of type + length (header included) + PNG data,
/// all big-endian.
fn icns(source: &RgbaImage) -> Vec<u8> {
    let mut body = Vec::new();
    for (kind, size) in ICNS_TYPES {
        let data = png(source, size);
        body.extend_from_slice(kind);
        body.extend_from_slice(&(8 + data.len() as u32).to_be_bytes());
        body.extend_from_slice(&data);
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"icns");
    out.extend_from_slice(&(8 + body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    out
}
