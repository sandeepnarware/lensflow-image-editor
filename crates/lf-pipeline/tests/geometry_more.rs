//! Geometry through the renderer: the view (zoom/pan) and the crop override, and the turn
//! helpers the crop tool uses.

use image::RgbaImage;
use lf_fixtures::{load_png, root};
use lf_pipeline::Renderer;
use lf_pipeline::renderer::{DrawOptions, View};
use lf_pipeline::test_support::shared_gpu;
use lf_pipeline::texture::{Target, read_back};
use lf_settings::geometry::{
    Geometry, TurnDir, apply, output_to_source, turn_settings, turned_size,
};
use lf_settings::{Crop, Rotate, normalise};
use serde_json::json;

const WHOLE: Crop = Crop {
    x: 0.0,
    y: 0.0,
    w: 1.0,
    h: 1.0,
};

fn source(name: &str) -> RgbaImage {
    load_png(root().join(format!("sources/{name}.png"))).unwrap()
}

fn max_diff(a: &RgbaImage, b: &RgbaImage) -> u8 {
    assert_eq!(a.dimensions(), b.dimensions());
    a.as_raw()
        .iter()
        .zip(b.as_raw())
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0)
}

#[test]
fn view_quarter_matches_crop() {
    let gpu = shared_gpu();
    // Even sides, so the quarter is a whole number of output pixels.
    let src = image::imageops::crop_imm(&source("landscape"), 0, 0, 512, 340).to_image();
    let s = normalise(&json!({ "exposure": 0.4, "contrast": 20, "saturation": 15 }));
    let mut r = Renderer::new(gpu.clone(), &src, 512, 340).unwrap();
    let full = r.render(&s).unwrap();
    assert_eq!(full.dimensions(), (512, 340));
    let (w, h) = (256, 170);
    let canvas = Target::new(&gpu, w, h, false).unwrap();
    let opts = DrawOptions {
        view: View {
            x: 0.5,
            y: 0.5,
            w: 0.5,
            h: 0.5,
        },
        ..DrawOptions::default()
    };
    r.draw(&s, &opts, &canvas).unwrap();
    let got = read_back(&gpu, &canvas).unwrap();
    let want = image::imageops::crop_imm(&full, w, h, w, h).to_image();
    assert!(max_diff(&got, &want) <= 1, "the zoomed quarter differs");
}

#[test]
fn crop_override_sizes_output() {
    let gpu = shared_gpu();
    let src = source("portrait");
    let (sw, sh) = src.dimensions();
    let crop = json!({ "x": 0.125, "y": 0.25, "w": 0.5, "h": 0.375 });
    let cropped = normalise(&json!({ "rotate": 90, "crop": crop, "exposure": 0.3 }));
    let uncropped = normalise(&json!({ "rotate": 90, "exposure": 0.3 }));
    let mut r = Renderer::new(gpu.clone(), &src, sw, sh).unwrap();

    let turned = turned_size(sw, sh, Rotate::R90);
    assert_eq!(r.output_size(&uncropped), turned);
    assert_ne!(r.output_size(&cropped), turned);
    let want = r.render(&uncropped).unwrap();
    assert_eq!(want.dimensions(), turned);

    // The crop tool's view: the cropped settings with the whole frame drawn.
    let canvas = Target::new(&gpu, turned.0, turned.1, false).unwrap();
    let opts = DrawOptions {
        crop: Some(WHOLE),
        ..DrawOptions::default()
    };
    r.draw(&cropped, &opts, &canvas).unwrap();
    let got = read_back(&gpu, &canvas).unwrap();
    assert!(
        max_diff(&got, &want) <= 1,
        "the override is not the whole frame"
    );
}

#[test]
fn r180_and_turn_settings_round_trip() {
    let mut g = Geometry {
        rotate: Rotate::R180,
        flip_h: false,
        flip_v: false,
        angle: 0.0,
        crop: WHOLE,
    };
    let m = output_to_source(400, 300, &g, &WHOLE);
    let close =
        |(x, y): (f64, f64), (a, b): (f64, f64)| (x - a).abs() < 1e-12 && (y - b).abs() < 1e-12;
    assert!(close(apply(&m, 0.0, 0.0), (1.0, 1.0)));
    assert!(close(apply(&m, 1.0, 1.0), (0.0, 0.0)));
    assert!(close(apply(&m, 0.25, 0.75), (0.75, 0.25)));

    // Dyadic crop values keep the turns exact.
    g.crop = Crop {
        x: 0.125,
        y: 0.25,
        w: 0.5,
        h: 0.375,
    };
    g.flip_h = true;
    g.angle = 7.5;
    let mut t = g;
    for _ in 0..4 {
        t = turn_settings(&t, TurnDir::Clockwise);
    }
    assert_eq!(t, g, "four clockwise turns");
    let back = turn_settings(
        &turn_settings(&g, TurnDir::Clockwise),
        TurnDir::Anticlockwise,
    );
    assert_eq!(back, g, "clockwise then anticlockwise");
    let once = turn_settings(&g, TurnDir::Clockwise);
    assert_eq!(once.rotate, Rotate::R270);
    assert_ne!(once, g);
}
