//! The real models, end to end. They run only when `LF_AI_DIR` names a folder holding the
//! runtime and models (the app's own AI folder after it has downloaded them, or let these
//! tests download into it with `LF_AI_DOWNLOAD=1`); otherwise each test returns at once.

use lf_ai::assets::Asset;
use lf_ai::segment::Target;
use lf_ai::{Engine, Gray, Rgb};
use std::sync::atomic::AtomicBool;

fn engine(models: &[Asset]) -> Option<Engine> {
    let dir = std::env::var_os("LF_AI_DIR")?;
    let engine = Engine::new(dir);
    if engine.to_download(models) > 0 && std::env::var_os("LF_AI_DOWNLOAD").is_none() {
        eprintln!("skipped: models missing from LF_AI_DIR (set LF_AI_DOWNLOAD=1 to fetch them)");
        return None;
    }
    engine
        .prepare(models, &mut |_, _| {}, &AtomicBool::new(false))
        .expect("prepare");
    Some(engine)
}

/// A soft scene: a bright sky band over darker ground, with a red disc in the middle.
fn scene(w: usize, h: usize) -> Rgb {
    let mut img = Rgb::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let sky = y < h / 3;
            let disc =
                (x as f32 - w as f32 / 2.0).hypot(y as f32 - h as f32 * 0.6) < h as f32 / 6.0;
            let c = if disc {
                [0.8, 0.15, 0.1]
            } else if sky {
                [0.45, 0.65, 0.95]
            } else {
                [0.25, 0.3, 0.15]
            };
            img.data[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&c);
        }
    }
    img
}

fn noisy(img: &Rgb) -> Rgb {
    let mut out = img.clone();
    let mut seed = 12345u32;
    for v in &mut out.data {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
        *v = (*v + ((seed >> 16) as f32 / 65535.0 - 0.5) * 0.2).clamp(0.0, 1.0);
    }
    out
}

fn error(a: &Rgb, b: &Rgb) -> f32 {
    a.data
        .iter()
        .zip(&b.data)
        .map(|(x, y)| (x - y).abs())
        .sum::<f32>()
        / a.data.len() as f32
}

#[test]
fn denoising_moves_a_noisy_picture_towards_the_clean_one() {
    let Some(e) = engine(&lf_ai::restore::DENOISE_MODELS) else {
        return;
    };
    let clean = scene(300, 200);
    let dirty = noisy(&clean);
    let out =
        lf_ai::restore::denoise(&e, &dirty, 1.0, &mut |_, _| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!((out.w, out.h), (300, 200));
    assert!(error(&out, &clean) < error(&dirty, &clean) * 0.6);
}

#[test]
fn upscaling_doubles_and_quadruples() {
    let Some(e) = engine(&lf_ai::restore::UPSCALE_MODELS) else {
        return;
    };
    let img = scene(120, 80);
    for f in [2, 4] {
        let out =
            lf_ai::restore::upscale(&e, &img, f, &mut |_, _| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!((out.w, out.h), (120 * f, 80 * f));
        assert!(error(&out.resized(120, 80), &img) < 0.05);
    }
}

#[test]
fn the_sky_mask_finds_the_sky_band() {
    let Some(e) = engine(&lf_ai::segment::SKY_MODELS) else {
        return;
    };
    let m = lf_ai::segment::mask(&e, Target::Sky, &scene(480, 320), 48, 32).unwrap();
    let top = m.data[2 * 48 + 24];
    let bottom = m.data[30 * 48 + 24];
    assert!(top > bottom, "top {top} bottom {bottom}");
}

#[test]
fn the_subject_mask_is_sized_as_asked() {
    let Some(e) = engine(&lf_ai::segment::SUBJECT_MODELS) else {
        return;
    };
    let m = lf_ai::segment::mask(&e, Target::Subject, &scene(480, 320), 60, 40).unwrap();
    assert_eq!((m.w, m.h), (60, 40));
    assert!(m.data.iter().all(|v| (0.0..=1.0).contains(v)));
}

#[test]
fn removal_fills_a_painted_square_from_its_surroundings() {
    let Some(e) = engine(&lf_ai::inpaint::INPAINT_MODELS) else {
        return;
    };
    let mut img = scene(640, 480);
    let clean = img.clone();
    let mut mask = Gray {
        w: 640,
        h: 480,
        data: vec![0.0; 640 * 480],
    };
    // A black square on the ground, painted over.
    for y in 380..420 {
        for x in 100..140 {
            img.data[(y * 640 + x) * 3..(y * 640 + x) * 3 + 3].copy_from_slice(&[0.0, 0.0, 0.0]);
            mask.data[y * 640 + x] = 1.0;
        }
    }
    let out =
        lf_ai::inpaint::remove(&e, &img, &mask, &mut |_, _| {}, &AtomicBool::new(false)).unwrap();
    let ground = clean.at(120, 400);
    let got = out.at(120, 400);
    assert!((got[1] - ground[1]).abs() < 0.12, "{got:?} vs {ground:?}");
    assert_eq!(out.at(400, 50), img.at(400, 50), "far away changed");
}

#[test]
fn no_face_is_found_in_a_landscape() {
    let Some(e) = engine(&lf_ai::faces::FACE_MODELS) else {
        return;
    };
    assert!(
        lf_ai::faces::detect(&e, &scene(640, 427))
            .unwrap()
            .is_empty()
    );
}
