//! RAW through the render chain: headroom survives to the develop pass on the float
//! path, and not on the 8-bit path (which has none to give).

mod support {
    pub mod synthetic_dng;
}

use image::{Rgb, Rgb32FImage, Rgba, RgbaImage};
use lf_pipeline::test_support::try_shared_gpu;
use lf_pipeline::{Gpu, Renderer};
use lf_raw::color::XYZ_D65_TO_LINEAR_SRGB;
use lf_raw::demosaic::Quality;
use lf_raw::open::{Picture, open};
use lf_raw::render::renderer_for;
use lf_settings::{DevelopSettings, EditSettings};
use std::path::PathBuf;
use std::sync::Arc;

fn gpu() -> Option<Arc<Gpu>> {
    let g = try_shared_gpu();
    if g.is_none() {
        println!("skipped: no GPU adapter");
    }
    g
}

fn patch(x: u32, y: u32) -> bool {
    (16..32).contains(&x) && (16..32).contains(&y)
}

fn settings(highlights: f64) -> EditSettings {
    let mut s = EditSettings::default_settings();
    s.highlights = highlights;
    s
}

fn centre(img: &RgbaImage) -> [u8; 3] {
    let p = img.get_pixel(24, 24).0;
    [p[0], p[1], p[2]]
}

fn temp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("lf-raw-chain-{}-{name}", std::process::id()))
}

#[test]
fn headroom_is_recoverable_by_the_chain() {
    let Some(gpu) = gpu() else { return };

    // RAW: grey with a patch at linear 3.0.
    let linear = Rgb32FImage::from_fn(48, 48, |x, y| {
        let v = if patch(x, y) { 3.0 } else { 0.18 };
        Rgb([v, v, v])
    });
    let path = temp("scene.dng");
    support::synthetic_dng::write(
        &path,
        &linear,
        support::synthetic_dng::RGGB,
        64,
        16383,
        [2.0, 1.0, 1.5],
        XYZ_D65_TO_LINEAR_SRGB,
    );
    let opened = open(&path, Quality::Best, &DevelopSettings::default(), 8192).unwrap();
    let _ = std::fs::remove_file(&path);
    assert!(matches!(opened.picture, Picture::Float(_)));
    let mut float = renderer_for(&gpu, &opened).unwrap();
    let plain = centre(&float.render(&settings(0.0)).unwrap());
    let pulled = centre(&float.render(&settings(-100.0)).unwrap());
    println!("RAW patch: default {plain:?}, highlights -100 {pulled:?}");
    assert_eq!(plain, [255; 3]);
    assert!(pulled.iter().all(|&v| v < 255), "{pulled:?}");
    let spread = pulled.iter().max().unwrap() - pulled.iter().min().unwrap();
    assert!(spread <= 2, "{pulled:?}");

    // The same scene as an 8-bit PNG: the patch is already 255, and stays there.
    let grey = (0.18f32.powf(1.0 / 2.2) * 255.0).round() as u8;
    let png = RgbaImage::from_fn(48, 48, |x, y| {
        let v = if patch(x, y) { 255 } else { grey };
        Rgba([v, v, v, 255])
    });
    let path = temp("scene.png");
    png.save(&path).unwrap();
    let opened = open(&path, Quality::Best, &DevelopSettings::default(), 8192).unwrap();
    let _ = std::fs::remove_file(&path);
    assert!(matches!(opened.picture, Picture::EightBit(_)));
    let mut eight = renderer_for(&gpu, &opened).unwrap();
    let kept = centre(&eight.render(&settings(-100.0)).unwrap());
    println!("PNG patch: highlights -100 {kept:?}");
    // The develop pass scales highlight luma by 1 + highlights * 0.3 (0.7 at -100), so a
    // patch at white pulls to about 0.7 * 255 = 178 and no further: it has no headroom to
    // recover. The RAW patch kept its brightness above white, so it lands higher.
    assert!(kept.iter().all(|&v| (170..=186).contains(&v)), "{kept:?}");
    assert!(pulled[0] > kept[0], "{pulled:?} vs {kept:?}");
    let _: &Renderer = &eight;
}

#[test]
fn a_corpus_file_renders_through_the_float_path() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/raw-samples/files");
    let smallest = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_file())
        .min_by_key(|e| e.metadata().map(|m| m.len()).unwrap_or(u64::MAX));
    let Some(smallest) = smallest else {
        println!("skipped: no RAW samples (run scripts/fetch-raw-samples.sh)");
        return;
    };
    let Some(gpu) = gpu() else { return };
    let opened = open(
        &smallest.path(),
        Quality::Fast,
        &DevelopSettings::default(),
        2048,
    )
    .unwrap();
    assert!(matches!(opened.picture, Picture::Float(_)));
    let mut r = renderer_for(&gpu, &opened).unwrap();
    let s = EditSettings::default_settings();
    let out = r.render(&s).unwrap();
    let (ow, oh) = r.output_size(&s);
    let (w, h) = out.dimensions();
    let long = opened.full_width.max(opened.full_height).min(2048);
    let scale = f64::from(long) / f64::from(opened.full_width.max(opened.full_height));
    assert_eq!(
        (w, h),
        (
            (f64::from(ow) * scale).round() as u32,
            (f64::from(oh) * scale).round() as u32
        ),
        "{}",
        smallest.path().display()
    );
    let mean = out
        .pixels()
        .flat_map(|p| p.0[..3].iter().copied())
        .map(f64::from)
        .sum::<f64>()
        / (f64::from(w * h) * 3.0 * 255.0);
    println!("{}: {w}x{h}, mean {mean:.3}", smallest.path().display());
    assert!((0.05..0.95).contains(&mean), "mean {mean}");
}
