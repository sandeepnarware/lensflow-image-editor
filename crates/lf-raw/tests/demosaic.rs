mod support {
    pub mod psnr;
    pub mod synthetic_dng;
}

use image::{Rgb, Rgb32FImage};
use lf_raw::decode::decode_raw;
use lf_raw::demosaic::{self, LinearRgb, Quality};
use lf_raw::{Cfa, Color, Orientation, RawFrame, Rect};
use std::f32::consts::TAU;
use std::time::Instant;
use support::psnr::psnr;
use support::synthetic_dng;

const IDENTITY: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
const WB: [f32; 3] = [2.0, 1.0, 1.5];
const BLACK: u16 = 64;
const WHITE: u16 = 16383;

/// Smooth colour: per-channel gradients and slow ripples, plus a shared luminance
/// ripple with a period of ~20 pixels (as in natural images, the channels' fine
/// detail is correlated).
fn smooth_rgb(w: u32, h: u32) -> Rgb32FImage {
    Rgb32FImage::from_fn(w, h, |x, y| {
        let (xf, yf) = (x as f32, y as f32);
        let detail = 0.08 * (TAU * xf / 21.0).sin() * (TAU * yf / 19.0).cos();
        Rgb([
            0.3 + 0.8 * xf / w as f32 + 0.1 * (TAU * yf / 97.0).sin() + 2.0 * detail,
            0.3 + 0.4 * yf / h as f32 + 0.08 * (TAU * (xf + yf) / 83.0).sin() + detail,
            0.25 + 0.6 * (xf + yf) / (w + h) as f32 + 0.1 * (TAU * xf / 71.0).cos() + 1.5 * detail,
        ])
    })
}

/// A neutral vertical step edge, slightly slanted so every CFA phase meets it.
fn edge_column(w: u32, y: u32) -> f32 {
    w as f32 / 2.0 + y as f32 / 16.0
}

fn edge_rgb(w: u32, h: u32) -> Rgb32FImage {
    Rgb32FImage::from_fn(w, h, |x, y| {
        let v = if (x as f32) < edge_column(w, y) {
            0.1
        } else {
            0.8
        };
        Rgb([v, v, v])
    })
}

/// What the demosaic should recover: the source in camera space (divided by WB).
fn camera_space(img: &Rgb32FImage) -> Vec<f32> {
    img.pixels()
        .flat_map(|p| (0..3).map(move |c| (p.0[c] / WB[c]).clamp(0.0, 1.0)))
        .collect()
}

fn decode_with_blacks(
    name: &str,
    img: &Rgb32FImage,
    pattern: [u8; 4],
    blacks: [u16; 4],
) -> RawFrame {
    let path =
        std::env::temp_dir().join(format!("lf-raw-demosaic-{}-{name}.dng", std::process::id()));
    synthetic_dng::write_with_blacks(&path, img, pattern, blacks, WHITE, WB, IDENTITY);
    let raw = decode_raw(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    raw
}

fn develop_with_blacks(
    name: &str,
    img: &Rgb32FImage,
    pattern: [u8; 4],
    blacks: [u16; 4],
    q: Quality,
) -> LinearRgb {
    let raw = decode_with_blacks(name, img, pattern, blacks);
    let out = demosaic::run(&raw, q);
    assert_eq!((out.width, out.height), img.dimensions());
    assert_eq!(out.data.len(), (out.width * out.height * 3) as usize);
    out
}

fn develop(name: &str, img: &Rgb32FImage, pattern: [u8; 4], q: Quality) -> LinearRgb {
    develop_with_blacks(name, img, pattern, [BLACK; 4], q)
}

fn smooth_psnr(q: Quality, pattern: [u8; 4], tag: &str) -> f64 {
    let img = smooth_rgb(256, 192);
    let out = develop(&format!("smooth-{tag}-{q:?}"), &img, pattern, q);
    let p = psnr(&out.data, &camera_space(&img), 256, 192, 4);
    println!("demosaic {q:?} {tag} smooth PSNR: {p:.2} dB");
    p
}

#[test]
fn fast_reconstructs_a_smooth_image_above_40_db() {
    assert!(smooth_psnr(Quality::Fast, synthetic_dng::RGGB, "rggb") > 40.0);
}

#[test]
fn best_reconstructs_a_smooth_image_above_45_db() {
    assert!(smooth_psnr(Quality::Best, synthetic_dng::RGGB, "rggb") > 45.0);
}

#[test]
fn other_bayer_phases_reconstruct_too() {
    for (pattern, tag) in [([1, 0, 2, 1], "grbg"), ([2, 1, 1, 0], "bggr")] {
        assert!(smooth_psnr(Quality::Fast, pattern, tag) > 40.0);
        assert!(smooth_psnr(Quality::Best, pattern, tag) > 45.0);
    }
}

#[test]
fn black_is_subtracted_per_cfa_position() {
    let (w, h) = (64u32, 48u32);
    let img = smooth_rgb(w, h);
    let uniform = decode_with_blacks("black-uniform", &img, synthetic_dng::RGGB, [BLACK; 4]);
    let spread = [0, 2000, 4000, 6000];
    let mixed = decode_with_blacks("black-mixed", &img, synthetic_dng::RGGB, spread);
    assert_eq!(mixed.black, [0.0, 2000.0, 4000.0, 6000.0]);
    // Negative control: the same samples with one (mean) black for every position.
    let mut wrong = mixed.clone();
    wrong.black = [3000.0; 4];

    // Full frame, then an odd crop origin so the CFA position parity runs through the crop.
    let crops = [
        Rect { x: 0, y: 0, w, h },
        Rect {
            x: 1,
            y: 1,
            w: w - 1,
            h: h - 1,
        },
    ];
    for crop in crops {
        let run = |raw: &RawFrame| {
            let mut raw = raw.clone();
            raw.crop = crop;
            demosaic::run(&raw, Quality::Fast)
        };
        let (cw, ch) = (crop.w as usize, crop.h as usize);
        let reference = run(&uniform);
        let right = psnr(&reference.data, &run(&mixed).data, cw, ch, 0);
        let off = psnr(&reference.data, &run(&wrong).data, cw, ch, 0);
        println!(
            "black per position, crop {crop:?}: {right:.2} dB; one black for all: {off:.2} dB"
        );
        assert!(right > 70.0, "per-position black: {right} dB");
        assert!(off < 70.0, "negative control should fail: {off} dB");
    }
}

#[test]
fn best_has_less_error_than_fast_on_a_sharp_edge() {
    let (w, h) = (128u32, 96u32);
    let img = edge_rgb(w, h);
    let want = camera_space(&img);
    let mae = |q: Quality| {
        let out = develop(&format!("edge-{q:?}"), &img, synthetic_dng::RGGB, q);
        let (mut sum, mut n) = (0.0f64, 0usize);
        for y in 4..h - 4 {
            let edge = edge_column(w, y) as u32;
            for x in edge - 3..=edge + 3 {
                for c in 0..3 {
                    let i = ((y * w + x) * 3 + c) as usize;
                    sum += (out.data[i] - want[i]).abs() as f64;
                    n += 1;
                }
            }
        }
        sum / n as f64
    };
    let (fast, best) = (mae(Quality::Fast), mae(Quality::Best));
    println!("demosaic edge MAE: Fast {fast:.5}, Best {best:.5}");
    assert!(best < fast, "Best {best} should beat Fast {fast}");
}

fn frame(w: u32, h: u32, cfa: Cfa, data: Vec<u16>) -> RawFrame {
    RawFrame {
        width: w,
        height: h,
        cfa,
        data,
        black: [100.0; 4],
        white: 1100.0,
        wb: [1.0; 4],
        xyz_to_cam: None,
        matrix_white: lf_raw::color::D65_WHITE,
        crop: Rect { x: 0, y: 0, w, h },
        orientation: Orientation::from_exif(1),
        make: String::new(),
        model: String::new(),
    }
}

#[test]
fn rgb_frames_pass_through_normalised_and_unclamped() {
    let mut raw = frame(2, 1, Cfa::Rgb, vec![100, 600, 1100, 1600, 350, 100]);
    raw.crop = Rect {
        x: 1,
        y: 0,
        w: 1,
        h: 1,
    };
    let out = demosaic::run(&raw, Quality::Best);
    assert_eq!((out.width, out.height), (1, 1));
    assert_eq!(out.data, vec![1.5, 0.25, 0.0]);
}

#[test]
fn output_is_the_crop_rectangle() {
    let (w, h) = (40u32, 30u32);
    let pattern = [[Color::Red, Color::Green], [Color::Green, Color::Blue]];
    let mut raw = frame(w, h, Cfa::Bayer { pattern }, vec![600; (w * h) as usize]);
    raw.crop = Rect {
        x: 3,
        y: 5,
        w: 20,
        h: 10,
    };
    for q in [Quality::Fast, Quality::Best] {
        let out = demosaic::run(&raw, q);
        assert_eq!((out.width, out.height), (20, 10));
        assert!(out.data.iter().all(|v| (v - 0.5).abs() < 1e-5), "{q:?}");
    }
}

#[test]
fn xtrans_flat_field_reconstructs_exactly() {
    use Color::{Blue as B, Green as G, Red as R};
    let pattern = [
        [G, G, R, G, G, B],
        [G, G, B, G, G, R],
        [B, R, G, R, B, G],
        [G, G, B, G, G, R],
        [G, G, R, G, G, B],
        [R, B, G, B, R, G],
    ];
    let (w, h) = (36u32, 24u32);
    let value = |c: Color| match c {
        R => 300u16,
        G => 600,
        B => 850,
    };
    let data = (0..h)
        .flat_map(|y| (0..w).map(move |x| value(pattern[(y % 6) as usize][(x % 6) as usize])))
        .collect();
    let raw = frame(w, h, Cfa::XTrans { pattern }, data);
    for q in [Quality::Fast, Quality::Best] {
        let out = demosaic::run(&raw, q);
        for px in out.data.chunks(3) {
            assert!((px[0] - 0.2).abs() < 1e-5, "{q:?} {px:?}");
            assert!((px[1] - 0.5).abs() < 1e-5, "{q:?} {px:?}");
            assert!((px[2] - 0.75).abs() < 1e-5, "{q:?} {px:?}");
        }
    }
}

/// Printed, not asserted: the aim is a 24 MP frame under 10 s with --release.
/// Debug builds use a 4 MP frame; set LF_DEMOSAIC_MP to override.
#[test]
fn timing_of_best_on_a_large_frame() {
    let mp: f64 = std::env::var("LF_DEMOSAIC_MP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(if cfg!(debug_assertions) { 4.0 } else { 24.0 });
    let w = ((mp * 1e6 * 1.5).sqrt() as u32) & !1;
    let h = ((w as f64 / 1.5) as u32) & !1;
    let pattern = [[Color::Red, Color::Green], [Color::Green, Color::Blue]];
    let data = (0..h)
        .flat_map(|y| (0..w).map(move |x| 100 + ((x * 7 + y * 13) % 1000) as u16))
        .collect();
    let raw = frame(w, h, Cfa::Bayer { pattern }, data);
    let t = Instant::now();
    let out = demosaic::run(&raw, Quality::Best);
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    println!(
        "demosaic Best {w}x{h} ({:.1} MP, {profile}): {:.2} s",
        (w * h) as f64 / 1e6,
        t.elapsed().as_secs_f64()
    );
    assert_eq!(out.data.len(), (w * h * 3) as usize);
}

#[test]
fn a_cropped_develop_matches_the_same_region_of_a_full_one() {
    let img = smooth_rgb(100, 90);
    let path =
        std::env::temp_dir().join(format!("lf-raw-demosaic-{}-crop.dng", std::process::id()));
    synthetic_dng::write(&path, &img, synthetic_dng::RGGB, BLACK, WHITE, WB, IDENTITY);
    let mut raw = decode_raw(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    for q in [Quality::Fast, Quality::Best] {
        let full = demosaic::run(&raw, q);
        raw.crop = Rect {
            x: 3,
            y: 37,
            w: 60,
            h: 41,
        };
        let part = demosaic::run(&raw, q);
        raw.crop = Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 90,
        };
        for y in 0..41 {
            for x in 0..60 {
                let a = &part.data[(y * 60 + x) * 3..][..3];
                let b = &full.data[((y + 37) * 100 + x + 3) * 3..][..3];
                assert_eq!(a, b, "{q:?} at {x},{y}");
            }
        }
    }
}
