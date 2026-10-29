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

#[test]
fn a_landscape_is_scored_sharp_with_no_faces() {
    let Some(engine) = engine(&lf_ai::cull::CULL_MODELS) else {
        return;
    };
    let mut scorer = lf_ai::cull::Scorer::new(&engine).expect("scorer");
    let score = scorer.score(&noisy(&scene(640, 480))).expect("score");
    assert_eq!(score.faces, 0);
    assert_eq!(score.eyes_closed, 0);
    assert_eq!(score.eye_sharpness, None);
}

#[test]
fn people_masks_are_sized_as_asked_and_empty_in_a_landscape() {
    let Some(engine) = engine(&lf_ai::segment::PEOPLE_MODELS) else {
        return;
    };
    let img = scene(640, 480);
    for target in [
        Target::People,
        Target::Hair,
        Target::FaceSkin,
        Target::Clothes,
    ] {
        let mask = lf_ai::segment::mask(&engine, target, &img, 320, 240).expect("mask");
        assert_eq!((mask.w, mask.h), (320, 240));
        let mean = mask.data.iter().sum::<f32>() / mask.data.len() as f32;
        assert!(mean < 0.2, "{target:?}: {mean}");
    }
}

#[test]
fn faces_are_described_as_unit_vectors_that_tell_pictures_apart() {
    let Some(engine) = engine(&lf_ai::identity::IDENTITY_MODELS) else {
        return;
    };
    let mut id = lf_ai::identity::Identifier::new(&engine).expect("identifier");
    let img = noisy(&scene(640, 480));
    assert!(
        id.find(&img).expect("find").is_empty(),
        "a landscape has no one in it"
    );
    // A made-up face box (the eyes 40 px apart) at two places in the scene.
    let face_at = |x: f32, y: f32| lf_ai::faces::Face {
        x,
        y,
        w: 100.0,
        h: 120.0,
        score: 0.9,
        landmarks: [
            [x + 30.0, y + 50.0],
            [x + 70.0, y + 50.0],
            [x + 50.0, y + 70.0],
            [x + 35.0, y + 90.0],
            [x + 65.0, y + 90.0],
        ],
    };
    let a = id.describe(&img, &face_at(100.0, 100.0)).expect("describe");
    assert_eq!(a.len(), lf_ai::identity::LENGTH);
    let norm = a.iter().map(|v| v * v).sum::<f32>().sqrt();
    assert!((norm - 1.0).abs() < 1e-4, "unit length: {norm}");
    let again = id.describe(&img, &face_at(100.0, 100.0)).expect("describe");
    assert!(
        lf_ai::identity::cosine(&a, &again) > 0.9999,
        "the same input"
    );
    let other = id.describe(&img, &face_at(400.0, 300.0)).expect("describe");
    assert!(lf_ai::identity::cosine(&a, &other) < 0.99, "another place");
}

#[test]
fn the_landscape_fixture_is_keyworded_as_a_landscape() {
    let Some(engine) = engine(&lf_ai::keywords::KEYWORD_MODELS) else {
        return;
    };
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/sources/landscape.png"
    );
    let img = Rgb::from_rgba8(&image::open(path).expect("fixture").to_rgba8());
    let mut tagger = lf_ai::keywords::Tagger::new(&engine).expect("tagger");
    let words: Vec<&str> = tagger
        .tag(&img)
        .expect("tag")
        .iter()
        .map(|s| s.word)
        .collect();
    assert!(!words.is_empty() && words.len() <= lf_ai::keywords::MAX_WORDS);
    assert!(
        words
            .iter()
            .any(|w| ["hills", "landscape", "countryside", "field", "mountains"].contains(w)),
        "{words:?}"
    );
}

#[test]
fn a_click_on_a_disc_masks_the_disc() {
    let Some(engine) = engine(&lf_ai::objects::OBJECT_MODELS) else {
        return;
    };
    // A bright orange disc on a dark blue-grey ground.
    let (w, h) = (640, 480);
    let mut img = Rgb::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let inside = (x as f32 - 400.0).hypot(y as f32 - 220.0) < 110.0;
            let c = if inside {
                [0.95, 0.55, 0.1]
            } else {
                [0.15, 0.18, 0.25]
            };
            img.data[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&c);
        }
    }
    let mask = lf_ai::objects::object_mask(&engine, &img, (400.0 / 640.0, 220.0 / 480.0), 320, 240)
        .expect("mask");
    assert_eq!((mask.w, mask.h), (320, 240));
    let at = |x: usize, y: usize| mask.data[y * 320 + x];
    assert!(at(200, 110) > 0.8, "the disc's middle: {}", at(200, 110));
    assert!(at(10, 10) < 0.2, "a corner: {}", at(10, 10));
    assert!(at(60, 200) < 0.2, "the ground: {}", at(60, 200));
}

/// Needs a real photo of a face (the fixtures have none): `LF_AI_FACE_PHOTO` names one.
#[test]
fn a_faces_eyes_are_above_its_lips() {
    let Some(engine) = engine(&lf_ai::features::FEATURE_MODELS) else {
        return;
    };
    let Some(path) = std::env::var_os("LF_AI_FACE_PHOTO") else {
        eprintln!("skipped: set LF_AI_FACE_PHOTO to a photo of a face");
        return;
    };
    let img = Rgb::from_rgba8(&image::open(path).expect("photo").to_rgba8());
    let (w, h) = (img.w / 2, img.h / 2);
    let centre_y = |m: &lf_ai::Gray| {
        let (mut sum, mut n) = (0.0f32, 0.0f32);
        for (i, v) in m.data.iter().enumerate() {
            sum += v * (i / m.w) as f32;
            n += v;
        }
        (n > 0.0).then(|| sum / n)
    };
    let eyes = lf_ai::segment::mask(&engine, Target::Eyes, &img, w, h).expect("eyes");
    let lips = lf_ai::segment::mask(&engine, Target::Lips, &img, w, h).expect("lips");
    let irises = lf_ai::segment::mask(&engine, Target::Irises, &img, w, h).expect("irises");
    let (Some(ey), Some(ly)) = (centre_y(&eyes), centre_y(&lips)) else {
        panic!("no eyes or no lips found in the portrait");
    };
    assert!(ey < ly, "eyes at {ey}, lips at {ly}");
    let area = |m: &lf_ai::Gray| m.data.iter().sum::<f32>();
    assert!(
        area(&irises) > 0.0 && area(&irises) < area(&eyes),
        "irises sit in the eyes"
    );
    assert!(
        area(&lips) < (w * h) as f32 * 0.05,
        "lips are small: {}",
        area(&lips)
    );
}

#[test]
fn the_landscape_fixtures_sky_is_far_and_blurred_while_the_foreground_stays_sharp() {
    let Some(engine) = engine(&lf_ai::depth::DEPTH_MODELS) else {
        return;
    };
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/sources/landscape.png"
    );
    let img = Rgb::from_rgba8(&image::open(path).expect("fixture").to_rgba8());
    let depth = lf_ai::depth::depth(&engine, &img, img.w, img.h).expect("depth");
    let band = |m: &lf_ai::Gray, y0: usize, y1: usize| {
        let rows = &m.data[y0 * m.w..y1 * m.w];
        rows.iter().sum::<f32>() / rows.len() as f32
    };
    let (top, bottom) = (
        band(&depth, 0, img.h / 5),
        band(&depth, img.h * 4 / 5, img.h),
    );
    assert!(
        bottom > top + 0.3,
        "near at the bottom: top {top}, bottom {bottom}"
    );
    // Focus on the foreground: the far half is softened, the near band kept.
    let focus = Some(bottom);
    let out = lf_ai::lens_blur::lens_blur(&img, &depth, 1.0, 0.2, focus);
    let detail = |p: &Rgb, y0: usize, y1: usize| {
        let mut s = 0.0f32;
        for y in y0..y1 {
            for x in 1..p.w {
                s += (p.at(x, y)[1] - p.at(x - 1, y)[1]).abs();
            }
        }
        s
    };
    let (h, w) = (img.h, img.w);
    let _ = w;
    let near = (h * 85 / 100, h * 95 / 100);
    let far = (h * 30 / 100, h * 45 / 100);
    assert!(
        detail(&out, near.0, near.1) > detail(&img, near.0, near.1) * 0.8,
        "the near band keeps its detail"
    );
    assert!(
        detail(&out, far.0, far.1) < detail(&img, far.0, far.1) * 0.8,
        "the far band is softened"
    );
}

/// Lens blur's cost on a 24-megapixel photo (prints the time; only with `LF_AI_DIR` set).
#[test]
fn lens_blur_on_24_megapixels_takes_seconds_not_minutes() {
    if std::env::var_os("LF_AI_DIR").is_none() {
        return;
    }
    let (w, h) = (6000, 4000);
    let mut img = Rgb::new(w, h);
    for (i, v) in img.data.iter_mut().enumerate() {
        *v = ((i * 7919) % 1000) as f32 / 1000.0;
    }
    let depth = lf_ai::Gray {
        w,
        h,
        data: (0..w * h).map(|i| (i / w) as f32 / h as f32).collect(),
    };
    let t = std::time::Instant::now();
    let out = lf_ai::lens_blur::lens_blur(&img, &depth, 1.0, 0.2, Some(1.0));
    let took = t.elapsed();
    eprintln!("lens blur, 24 MP, full amount: {took:?}");
    assert_eq!((out.w, out.h), (w, h));
}

#[test]
fn the_landscape_fixtures_foreground_mask_is_at_the_bottom() {
    let Some(engine) = engine(&lf_ai::depth::DEPTH_MODELS) else {
        return;
    };
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/sources/landscape.png"
    );
    let img = Rgb::from_rgba8(&image::open(path).expect("fixture").to_rgba8());
    let (w, h) = (img.w / 2, img.h / 2);
    let fg = lf_ai::segment::mask(&engine, Target::Foreground, &img, w, h).expect("mask");
    let bg = lf_ai::segment::mask(&engine, Target::Background, &img, w, h).expect("mask");
    let at = |m: &lf_ai::Gray, y: usize| m.data[y * w + w / 2];
    assert!(at(&fg, h - 2) > 0.9 && at(&fg, 2) < 0.1, "foreground below");
    assert!(at(&bg, 2) > 0.9 && at(&bg, h - 2) < 0.1, "background above");
}

/// Remove objects' "Select people": on a real portrait (`LF_AI_FACE_PHOTO`), the person is
/// selected and the room behind is not.
#[test]
fn select_people_covers_the_person_on_a_real_portrait() {
    let Some(engine) = engine(&lf_ai::segment::PEOPLE_MODELS) else {
        return;
    };
    let Some(path) = std::env::var_os("LF_AI_FACE_PHOTO") else {
        return;
    };
    let img = Rgb::from_rgba8(&image::open(path).expect("photo").to_rgba8());
    let (w, h) = (img.w / 2, img.h / 2);
    let m = lf_ai::segment::mask(&engine, Target::People, &img, w, h).expect("mask");
    let at = |x: usize, y: usize| m.data[y * w + x];
    assert!(at(w / 2, h / 2) > 0.5, "the person's middle");
    assert!(at(3, 3) < 0.5 && at(w - 4, 3) < 0.5, "the top corners");
}

#[test]
fn the_landscape_fixtures_sky_is_replaced_and_its_land_kept() {
    let Some(engine) = engine(&lf_ai::sky::SKY_MODELS) else {
        return;
    };
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/sources/landscape.png"
    );
    let img = Rgb::from_rgba8(&image::open(path).expect("fixture").to_rgba8());
    let mut sky = Rgb::new(64, 32);
    for p in sky.data.chunks_mut(3) {
        p.copy_from_slice(&[0.95, 0.55, 0.15]);
    }
    let out = lf_ai::sky::replace(&engine, &img, &sky).expect("replace");
    let (w, h) = (img.w, img.h);
    let top = out.at(w / 2, 2);
    assert!(
        top[0] > 0.8 && top[2] < 0.3,
        "the new sky at the top: {top:?}"
    );
    let (a, b) = (img.at(w / 2, h - 3), out.at(w / 2, h - 3));
    for c in 0..3 {
        assert!(
            (a[c] - b[c]).abs() < 0.12,
            "the land stays near itself: {a:?} -> {b:?}"
        );
    }
}
