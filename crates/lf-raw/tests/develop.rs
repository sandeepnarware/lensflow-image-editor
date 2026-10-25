mod support {
    pub mod synthetic_dng;
}

use half::f16;
use image::{Rgb, Rgb32FImage, Rgba, RgbaImage};
use lf_raw::color::{self, XYZ_D65_TO_LINEAR_SRGB};
use lf_raw::decode::decode_raw;
use lf_raw::demosaic::Quality;
use lf_raw::develop::{Developed, develop, develop_rgb8};
use lf_raw::{Cfa, Color, Orientation, RawFrame, Rect, curve};
use lf_settings::{DevelopSettings, Highlights, Profile};
use std::f32::consts::TAU;

const WB: [f32; 3] = [2.0, 1.0, 1.5];
const BLACK: u16 = 64;
const WHITE: u16 = 16383;
/// A ColorMatrix that develops to the identity: XYZ -> camera equal to XYZ -> sRGB.
const SRGB_CAMERA: [[f32; 3]; 3] = XYZ_D65_TO_LINEAR_SRGB;

const LINEAR_CLIP: DevelopSettings = DevelopSettings {
    profile: Profile::Linear,
    highlights: Highlights::Clip,
};

fn dng(name: &str, img: &Rgb32FImage) -> RawFrame {
    let path =
        std::env::temp_dir().join(format!("lf-raw-develop-{}-{name}.dng", std::process::id()));
    support::synthetic_dng::write(
        &path,
        img,
        support::synthetic_dng::RGGB,
        BLACK,
        WHITE,
        WB,
        SRGB_CAMERA,
    );
    let raw = decode_raw(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    raw
}

/// RGB of pixel (x, y) of the developed frame, as f32.
fn px(d: &Developed, x: u32, y: u32) -> [f32; 3] {
    let i = ((y * d.frame.width + x) * 4) as usize;
    [0, 1, 2].map(|c| d.frame.data[i + c].to_f32())
}

fn f16_of(v: f32) -> f32 {
    f16::from_f32(v).to_f32()
}

fn plain_frame(w: u32, h: u32, cfa: Cfa, data: Vec<u16>, white: f32) -> RawFrame {
    RawFrame {
        width: w,
        height: h,
        cfa,
        data,
        black: [0.0; 4],
        white,
        wb: [1.0; 4],
        xyz_to_cam: None,
        matrix_white: color::D65_WHITE,
        crop: Rect { x: 0, y: 0, w, h },
        orientation: Orientation::from_exif(1),
        make: String::new(),
        model: String::new(),
    }
}

/// A smooth 8-bit picture (gamma 2.2 values), in gamut for the sensor.
fn picture(w: u32, h: u32) -> RgbaImage {
    RgbaImage::from_fn(w, h, |x, y| {
        let (xf, yf) = (x as f32 / w as f32, y as f32 / h as f32);
        let v = |a: f32| (a.clamp(0.0, 1.0) * 255.0).round() as u8;
        Rgba([
            v(0.15 + 0.7 * xf),
            v(0.2 + 0.6 * yf + 0.05 * (TAU * xf * 2.0).sin()),
            v(0.3 + 0.4 * (xf + yf) / 2.0 + 0.05 * (TAU * yf * 3.0).cos()),
            255,
        ])
    })
}

#[test]
fn a_known_srgb_picture_round_trips_through_a_synthetic_dng() {
    let (w, h) = (96, 64);
    let src = picture(w, h);
    let linear = Rgb32FImage::from_fn(w, h, |x, y| {
        let p = src.get_pixel(x, y).0;
        Rgb([0, 1, 2].map(|c| (p[c] as f32 / 255.0).powf(2.2)))
    });
    let raw = dng("roundtrip", &linear);
    let d = develop(&raw, Quality::Best, &LINEAR_CLIP, 8192).unwrap();
    assert_eq!((d.frame.width, d.frame.height), (w, h));
    assert_eq!((d.full_width, d.full_height), (w, h));
    assert!(!d.reduced && !d.orientation_applied);

    let (mut sum, mut n) = (0.0f64, 0usize);
    for y in 4..h - 4 {
        for x in 4..w - 4 {
            let got = px(&d, x, y);
            let want = src.get_pixel(x, y).0;
            for c in 0..3 {
                sum += (got[c] - want[c] as f32 / 255.0).abs() as f64;
                n += 1;
            }
        }
    }
    let mean = sum / n as f64 * 255.0;
    println!("round trip mean error: {mean:.3}/255");
    assert!(mean < 1.5, "mean error {mean}/255");
}

/// A mid-grey frame with an overexposed (linear 3.0, sensor-clipped) square in the middle.
/// Each test passes its own `name`: two tests writing one temp file race on Windows,
/// where a file mapped by the decoder cannot be rewritten.
fn overexposed(name: &str) -> RawFrame {
    let img = Rgb32FImage::from_fn(48, 48, |x, y| {
        let v = if (16..32).contains(&x) && (16..32).contains(&y) {
            3.0
        } else {
            0.18
        };
        Rgb([v, v, v])
    });
    dng(name, &img)
}

#[test]
fn an_overexposed_patch_stays_above_one() {
    let raw = overexposed("overexposed-reconstruct");
    for profile in [Profile::Camera, Profile::Linear] {
        let s = DevelopSettings {
            profile,
            highlights: Highlights::Reconstruct,
        };
        let d = develop(&raw, Quality::Best, &s, 8192).unwrap();
        let [r, g, b] = px(&d, 24, 24);
        println!("{profile:?} reconstructed patch: {r} {g} {b}");
        assert!(r > 1.0 && g > 1.0 && b > 1.0, "{r} {g} {b}");
        assert!((r - g).abs() < 0.02 && (g - b).abs() < 0.02, "{r} {g} {b}");
    }
}

#[test]
fn clip_mode_clamps_each_channel() {
    let raw = overexposed("overexposed-clip");
    for profile in [Profile::Camera, Profile::Linear] {
        let s = DevelopSettings {
            profile,
            highlights: Highlights::Clip,
        };
        let d = develop(&raw, Quality::Best, &s, 8192).unwrap();
        let max = d
            .frame
            .data
            .iter()
            .map(|v| v.to_f32())
            .fold(f32::MIN, f32::max);
        assert!(max <= 1.0, "{profile:?}: {max}");
        assert_eq!(px(&d, 24, 24), [1.0; 3]);
    }
}

#[test]
#[allow(non_snake_case)]
fn the_camera_curve_maps_mid_grey_to_sRGB_mid() {
    let gamma = |x: f32| x.max(0.0).powf(1.0 / 2.2);
    let mid = gamma(curve::camera(0.18));
    assert!((mid - 0.46).abs() <= 0.01, "{mid}");

    let mut prev = curve::camera(-0.1);
    let mut x = -0.1f32;
    while x < 4.0 {
        x += 0.0005;
        let y = curve::camera(x);
        assert!(y > prev, "not increasing at {x}: {prev} -> {y}");
        assert!(y - prev < 0.002, "jump at {x}: {prev} -> {y}");
        prev = y;
    }
    assert_eq!(curve::camera(0.0), 0.0);
    assert!((curve::camera(1.0) - 1.0).abs() < 1e-6);
    let (below, above) = (curve::camera(1.0 - 1e-4), curve::camera(1.0 + 1e-4));
    assert!((above - below).abs() < 1e-3, "{below} {above}");
    assert!(curve::camera(3.0) > 1.0);
    // Blacks are lifted a little, not crushed.
    assert!(curve::camera(0.01) >= 0.01);
    assert_eq!(curve::apply(Profile::Linear, 0.37), 0.37);
}

#[test]
fn large_frames_are_reduced_to_max_edge() {
    let (w, h) = (9000u32, 6000u32);
    let raw = plain_frame(
        w,
        h,
        Cfa::Bayer {
            pattern: [[Color::Red, Color::Green], [Color::Green, Color::Blue]],
        },
        vec![2000; (w * h) as usize],
        4000.0,
    );
    let d = develop(&raw, Quality::Fast, &LINEAR_CLIP, 4096).unwrap();
    assert!(d.reduced);
    assert!(d.frame.width.max(d.frame.height) <= 4096);
    assert_eq!((d.frame.width, d.frame.height), (3000, 2000));
    assert_eq!((d.full_width, d.full_height), (w, h));
    let want = 0.5f32.powf(1.0 / 2.2);
    for v in px(&d, 1500, 1000) {
        assert!((v - want).abs() < 0.002, "{v} vs {want}");
    }
}

#[test]
fn orientation_turns_the_buffer() {
    // 3x2 frame, white at (1, 0); orientation 6 (rotate 90 clockwise) gives 2x3.
    let mut data = vec![0u16; 3 * 2 * 3];
    data[3..6].copy_from_slice(&[1000, 1000, 1000]);
    let mut raw = plain_frame(3, 2, Cfa::Rgb, data, 1000.0);
    raw.orientation = Orientation::from_exif(6);
    let d = develop(&raw, Quality::Fast, &LINEAR_CLIP, 8192).unwrap();
    assert!(d.orientation_applied);
    assert_eq!((d.frame.width, d.frame.height), (2, 3));
    assert_eq!((d.full_width, d.full_height), (2, 3));
    // Top row, middle lands in the right column, middle row.
    assert_eq!(px(&d, 1, 1), [1.0; 3]);
    assert_eq!(px(&d, 0, 1), [0.0; 3]);
    // Alpha is opaque.
    assert!(d.frame.data.chunks(4).all(|p| p[3].to_f32() == 1.0));

    raw.orientation = Orientation::from_exif(1);
    let d = develop(&raw, Quality::Fast, &LINEAR_CLIP, 8192).unwrap();
    assert!(!d.orientation_applied);
    assert_eq!(px(&d, 1, 0), [1.0; 3]);
}

#[test]
fn eight_bit_pictures_keep_their_values() {
    let img = RgbaImage::from_fn(3, 2, |x, y| {
        Rgba([(x * 100) as u8, (y * 200) as u8, 7, 255])
    });
    let d = develop_rgb8(&img, Orientation::from_exif(1));
    assert_eq!(
        (d.frame.width, d.frame.height, d.full_width, d.full_height),
        (3, 2, 3, 2)
    );
    assert!(!d.reduced && !d.orientation_applied);
    for (i, v) in d.frame.data.iter().enumerate() {
        assert_eq!(v.to_f32(), f16_of(img.as_raw()[i] as f32 / 255.0));
    }
    // Rotated 180: the bottom-right pixel comes first.
    let d = develop_rgb8(&img, Orientation::from_exif(3));
    assert!(d.orientation_applied);
    assert_eq!(
        px(&d, 0, 0),
        [200.0 / 255.0, 200.0 / 255.0, 7.0 / 255.0].map(f16_of)
    );
}

#[test]
fn colour_matrix_maps_camera_white_to_srgb_white() {
    // A plausible camera ColorMatrix, as if for D65 and as if for illuminant A:
    // camera (1, 1, 1) maps to sRGB (1, 1, 1) either way.
    let m = [
        [0.6722, -0.0635, -0.0963],
        [-0.4287, 1.2460, 0.2028],
        [-0.0908, 0.2162, 0.5668],
    ];
    for white in [color::D65_WHITE, color::A_WHITE] {
        let c = color::cam_to_srgb(Some(m), white);
        for row in c {
            let sum: f32 = row.iter().sum();
            assert!((sum - 1.0).abs() < 1e-4, "{c:?}");
        }
    }
    // Illuminant A changes the matrix (Bradford adaptation happened).
    assert_ne!(
        color::cam_to_srgb(Some(m), color::D65_WHITE),
        color::cam_to_srgb(Some(m), color::A_WHITE)
    );
    let id = color::cam_to_srgb(Some(SRGB_CAMERA), color::D65_WHITE);
    for (i, row) in id.iter().enumerate() {
        for (j, v) in row.iter().enumerate() {
            let want = if i == j { 1.0 } else { 0.0 };
            assert!((v - want).abs() < 1e-4, "{id:?}");
        }
    }
    assert_eq!(color::cam_to_srgb(None, color::D65_WHITE), color::IDENTITY);
}

/// A Nikon D40 `ColorMatrix` (XYZ to camera, D65), as published in Adobe's DNG
/// converter tables: a real, far-from-identity matrix.
const D40_MATRIX: [[f32; 3]; 3] = [
    [0.6992, -0.1668, -0.0806],
    [-0.8138, 1.5748, 0.2543],
    [-0.0874, 0.0850, 0.7897],
];

/// A flat 32x32 RGGB frame (white 4095, black 0) whose R, G and B samples are
/// `cam` × white, developed with the D40 matrix and `wb`.
fn flat_d40(cam: [f32; 3], wb: [f32; 3], highlights: Highlights) -> [f32; 3] {
    let (w, h) = (32u32, 32u32);
    let mut data = vec![0u16; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let c = match (y & 1, x & 1) {
                (0, 0) => 0,
                (1, 1) => 2,
                _ => 1,
            };
            data[(y * w + x) as usize] = (cam[c] * 4095.0).round() as u16;
        }
    }
    let mut raw = plain_frame(
        w,
        h,
        Cfa::Bayer {
            pattern: [[Color::Red, Color::Green], [Color::Green, Color::Blue]],
        },
        data,
        4095.0,
    );
    raw.wb = [wb[0], wb[1], wb[2], wb[1]];
    raw.xyz_to_cam = Some(D40_MATRIX);
    let s = DevelopSettings {
        profile: Profile::Linear,
        highlights,
    };
    px(&develop(&raw, Quality::Best, &s, 8192).unwrap(), 16, 16)
}

/// The colour a camera value should develop to with no highlight handling at all:
/// white balance, matrix, gamma.
fn direct_d40(cam: [f32; 3], wb: [f32; 3]) -> [f32; 3] {
    let m = color::cam_to_srgb(Some(D40_MATRIX), color::D65_WHITE);
    let balanced = [cam[0] * wb[0], cam[1] * wb[1], cam[2] * wb[2]];
    color::apply(&m, balanced).map(|v| v.max(0.0).powf(1.0 / 2.2))
}

fn spread(c: [f32; 3]) -> f32 {
    c.iter().copied().fold(f32::MIN, f32::max) - c.iter().copied().fold(f32::MAX, f32::min)
}

#[test]
fn a_clipped_sensor_patch_develops_neutral_with_a_real_matrix() {
    // Every channel at white; daylight-like multipliers push R and B above 1.0 after
    // white balance, which (clamped after the matrix) used to come out magenta.
    let wb = [2.1, 1.0, 1.45];
    let clip = flat_d40([1.0; 3], wb, Highlights::Clip);
    println!("clipped patch, Clip: {clip:?}");
    assert!(spread(clip) < 0.01, "{clip:?}");
    assert!(clip.iter().all(|&v| (v - 1.0).abs() < 0.01), "{clip:?}");

    let rec = flat_d40([1.0; 3], wb, Highlights::Reconstruct);
    println!("clipped patch, Reconstruct: {rec:?}");
    assert!(spread(rec) < 0.01, "{rec:?}");
    assert!(rec.iter().all(|&v| v > 1.0), "{rec:?}");
}

#[test]
fn reconstruct_leaves_bright_unclipped_colours_alone() {
    // A saturated red well below sensor clipping.
    let cam = [0.45, 0.12, 0.05];

    // In-range after conversion: Reconstruct and Clip agree.
    let wb = [1.25, 1.0, 1.3];
    let (clip, rec) = (
        flat_d40(cam, wb, Highlights::Clip),
        flat_d40(cam, wb, Highlights::Reconstruct),
    );
    println!("unclipped red: Clip {clip:?}, Reconstruct {rec:?}");
    for c in 0..3 {
        assert!((clip[c] - rec[c]).abs() < 0.01, "{clip:?} vs {rec:?}");
    }

    // Strong multipliers take its red above 1.0 after the matrix, but the sensor never
    // clipped, so Reconstruct must not pull it toward grey (it used to blend by
    // (m - 1) / m * 2 on the converted maximum).
    let wb = [2.6, 1.0, 1.3];
    let want = direct_d40(cam, wb);
    let rec = flat_d40(cam, wb, Highlights::Reconstruct);
    println!("unclipped red, strong WB: want {want:?}, Reconstruct {rec:?}");
    assert!(want[0] > 1.0, "the case must leave the range: {want:?}");
    for c in 0..3 {
        assert!((want[c] - rec[c]).abs() < 0.01, "{want:?} vs {rec:?}");
    }
}

#[test]
fn preview_bins_a_bayer_frame_to_half_size() {
    let (w, h) = (96u32, 64u32);
    let src = picture(w, h);
    let linear = Rgb32FImage::from_fn(w, h, |x, y| {
        let p = src.get_pixel(x, y).0;
        Rgb([0, 1, 2].map(|c| (p[c] as f32 / 255.0).powf(2.2)))
    });
    let raw = dng("preview", &linear);
    let d = develop(&raw, Quality::Preview, &LINEAR_CLIP, 8192).unwrap();
    assert_eq!((d.frame.width, d.frame.height), (w / 2, h / 2));
    assert_eq!((d.full_width, d.full_height), (w, h));
    assert!(d.reduced);

    // Reference: the linear picture box-downscaled 2x2, gamma encoded.
    let (mut sum, mut n) = (0.0f64, 0usize);
    for y in 0..h / 2 {
        for x in 0..w / 2 {
            let got = px(&d, x, y);
            for (c, g) in got.iter().enumerate() {
                let mut m = 0.0f32;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    m += linear.get_pixel(2 * x + dx, 2 * y + dy).0[c];
                }
                let want = (m / 4.0).powf(1.0 / 2.2);
                sum += (g - want).abs() as f64;
                n += 1;
            }
        }
    }
    let mean = sum / n as f64 * 255.0;
    println!("preview vs 2x2 box reference: mean {mean:.3}/255");
    assert!(mean < 1.0, "mean error {mean}/255");
}

#[test]
fn preview_bins_x_trans_by_three_and_rgb_by_two() {
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
    let level = |c: Color| match c {
        R => 1000u16,
        G => 2000,
        B => 3000,
    };
    let data = (0..w * h)
        .map(|i| level(pattern[(i / w) as usize % 6][(i % w) as usize % 6]))
        .collect();
    let raw = plain_frame(w, h, Cfa::XTrans { pattern }, data, 4000.0);
    let d = develop(&raw, Quality::Preview, &LINEAR_CLIP, 8192).unwrap();
    assert_eq!((d.frame.width, d.frame.height), (12, 8));
    assert_eq!((d.full_width, d.full_height, d.reduced), (w, h, true));
    let want = [0.25f32, 0.5, 0.75].map(|v| f16_of(v.powf(1.0 / 2.2)));
    for (x, y) in [(0, 0), (5, 3), (11, 7)] {
        assert_eq!(px(&d, x, y), want, "({x}, {y})");
    }

    // An already-demosaiced frame: 2x2 means.
    let data = (0..4u32 * 2)
        .flat_map(|i| {
            let v = if i % 4 < 2 { 0 } else { 1000 };
            [v, v, v]
        })
        .collect();
    let raw = plain_frame(4, 2, Cfa::Rgb, data, 1000.0);
    let d = develop(&raw, Quality::Preview, &LINEAR_CLIP, 8192).unwrap();
    assert_eq!((d.frame.width, d.frame.height), (2, 1));
    assert_eq!(px(&d, 0, 0), [0.0; 3]);
    assert_eq!(px(&d, 1, 0), [1.0; 3]);
}
