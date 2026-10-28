mod support {
    pub mod synthetic_dng;
}

use image::{Rgb, Rgb32FImage};
use lf_raw::decode::decode_raw;
use lf_raw::{Cfa, Color, RawError};
use std::path::Path;
use support::synthetic_dng;

fn gradient_rgb(w: u32, h: u32) -> Rgb32FImage {
    Rgb32FImage::from_fn(w, h, |x, y| {
        Rgb([
            0.1 + 0.8 * x as f32 / w as f32,
            0.2 + 0.6 * y as f32 / h as f32,
            0.5,
        ])
    })
}

const IDENTITY: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

fn write_fixture(name: &str, img: &Rgb32FImage) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("lf-raw-{}-{name}.dng", std::process::id()));
    synthetic_dng::write(
        &path,
        img,
        synthetic_dng::RGGB,
        64,
        4095,
        [2.0, 1.0, 1.5],
        IDENTITY,
    );
    path
}

#[test]
fn decodes_a_synthetic_dng() {
    let img = gradient_rgb(64, 48);
    let path = write_fixture("decode", &img);
    let raw = decode_raw(&path).unwrap();
    assert_eq!((raw.width, raw.height), (64, 48));
    match raw.cfa {
        Cfa::Bayer { pattern } => {
            assert_eq!(
                pattern,
                [[Color::Red, Color::Green], [Color::Green, Color::Blue]]
            );
        }
        other => panic!("expected Bayer, got {other:?}"),
    }
    assert_eq!(raw.white, 4095.0);
    assert_eq!(raw.black, [64.0; 4]);
    assert!(
        (raw.wb[0] - 2.0).abs() < 1e-3
            && (raw.wb[1] - 1.0).abs() < 1e-3
            && (raw.wb[2] - 1.5).abs() < 1e-3
    );
    assert_eq!(raw.data.len(), 64 * 48);
    assert!(raw.xyz_to_cam.is_some());
    assert_eq!(
        (raw.make.as_str(), raw.model.as_str()),
        ("Synthetic", "Synthetic")
    );
    assert_eq!(raw.orientation.exif(), 1);

    // green site (row 0, col 1) holds the image's green scaled to the sensor range
    let want = 64.0 + img.get_pixel(1, 0).0[1] * (4095.0 - 64.0);
    assert!(
        (raw.data[1] as f32 - want).abs() <= 1.0,
        "{} vs {want}",
        raw.data[1]
    );
    // red site (row 2, col 4) undoes the WB multiplier of 2
    let want = 64.0 + img.get_pixel(4, 2).0[0] / 2.0 * (4095.0 - 64.0);
    assert!((raw.data[2 * 64 + 4] as f32 - want).abs() <= 1.0);
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_non_image_is_refused_with_its_name() {
    let e = decode_raw(Path::new("Cargo.toml")).unwrap_err();
    assert!(!matches!(e, RawError::Io { .. }));
    assert!(e.to_string().contains("Cargo.toml"), "{e}");
}

#[test]
fn a_missing_file_is_an_io_error_naming_the_path() {
    let e = decode_raw(Path::new("no-such-photo.dng")).unwrap_err();
    assert!(matches!(e, RawError::Io { .. }));
    assert!(e.to_string().contains("no-such-photo.dng"), "{e}");
}

#[test]
fn the_as_shot_colour_temperature_reads_from_the_headers() {
    // A "camera" whose RGB is linear sRGB: XYZ to camera is XYZ to sRGB.
    const XYZ_TO_SRGB: [[f32; 3]; 3] = [
        [3.2406, -1.5372, -0.4986],
        [-0.9689, 1.8758, 0.0415],
        [0.0557, -0.2040, 1.0570],
    ];
    let img = gradient_rgb(32, 24);
    let shoot = |name: &str, wb: [f32; 3]| {
        let path = std::env::temp_dir().join(format!("lf-raw-{}-{name}.dng", std::process::id()));
        synthetic_dng::write(&path, &img, synthetic_dng::RGGB, 64, 4095, wb, XYZ_TO_SRGB);
        let k = lf_raw::open::as_shot_kelvin(&path);
        let _ = std::fs::remove_file(&path);
        k.expect("a temperature")
    };
    // Balanced for D65 (sRGB white): about 6500 K.
    let daylight = shoot("d65", [1.0, 1.0, 1.0]);
    assert!((6000.0..7000.0).contains(&daylight), "{daylight}");
    // Balanced for warm light (red strong in the neutral, so a small red multiplier).
    let warm = shoot("warm", [0.55, 1.0, 2.4]);
    assert!(warm < 4000.0, "{warm}");
    // Not a RAW: none.
    let png = std::env::temp_dir().join(format!("lf-raw-{}-k.png", std::process::id()));
    image::RgbImage::new(2, 2).save(&png).unwrap();
    assert_eq!(lf_raw::open::as_shot_kelvin(&png), None);
    let _ = std::fs::remove_file(&png);
}
