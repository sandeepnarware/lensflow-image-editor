//! Lens corrections (desktop only): the develop pass samples where `Lens::to_source` says,
//! lifts the corners for lens vignetting and moves red and blue for chromatic aberration.

use image::{Rgba, RgbaImage};
use lf_pipeline::Renderer;
use lf_pipeline::test_support::shared_gpu;
use lf_settings::EditSettings;
use lf_settings::lens::Lens;

fn render(img: &RgbaImage, lens: Lens) -> RgbaImage {
    let mut s = EditSettings::default_settings();
    lens.write_into(&mut s);
    let mut r = Renderer::new(shared_gpu(), img, img.width(), img.height()).unwrap();
    r.render(&s).unwrap()
}

#[test]
fn the_gpu_samples_where_the_cpu_says() {
    // Red is x and green is y, so a pixel tells where it was sampled from.
    let img = RgbaImage::from_fn(256, 256, |x, y| Rgba([x as u8, y as u8, 0, 255]));
    let lens = Lens {
        distortion: 60.0,
        vertical: 40.0,
        horizontal: -30.0,
        ..Lens::default()
    };
    let out = render(&img, lens);
    for (x, y) in [(40u32, 40u32), (128, 128), (200, 60), (90, 210), (230, 230)] {
        let corrected = ((f64::from(x) + 0.5) / 256.0, (f64::from(y) + 0.5) / 256.0);
        let (su, sv) = lens.to_source(corrected, 1.0);
        if !(0.02..0.98).contains(&su) || !(0.02..0.98).contains(&sv) {
            continue;
        }
        let p = out.get_pixel(x, y).0;
        // The pipeline is not linear in value, but a ramp through identity edits is close:
        // compare positions to a few pixels.
        let (gu, gv) = (f64::from(p[0]) / 255.0, f64::from(p[1]) / 255.0);
        assert!(
            (gu - su).abs() < 0.02 && (gv - sv).abs() < 0.02,
            "({x}, {y}): gpu ({gu:.3}, {gv:.3}) cpu ({su:.3}, {sv:.3})"
        );
    }
    // And without corrections the pixel is its own.
    let plain = render(&img, Lens::default());
    let p = plain.get_pixel(200, 60).0;
    assert!((i32::from(p[0]) - 200).abs() <= 1 && (i32::from(p[1]) - 60).abs() <= 1);
}

#[test]
fn lens_vignetting_lifts_the_corners_and_leaves_the_centre() {
    let grey = RgbaImage::from_pixel(200, 140, Rgba([100, 100, 100, 255]));
    let plain = render(&grey, Lens::default());
    let lifted = render(
        &grey,
        Lens {
            vignetting: 80.0,
            ..Lens::default()
        },
    );
    let c = |img: &RgbaImage, x, y| img.get_pixel(x, y).0[1];
    assert!((i32::from(c(&lifted, 100, 70)) - i32::from(c(&plain, 100, 70))).abs() <= 1);
    assert!(c(&lifted, 2, 2) > c(&plain, 2, 2) + 20);
}

#[test]
fn chromatic_aberration_moves_red_and_blue_but_not_green() {
    // Bright stripes across: a channel moved in or out changes value at a stripe's edge.
    let stripes = RgbaImage::from_fn(240, 160, |x, _| {
        let v = if (x / 6) % 2 == 0 { 230 } else { 20 };
        Rgba([v, v, v, 255])
    });
    let plain = render(&stripes, Lens::default());
    let fixed = render(
        &stripes,
        Lens {
            ca_red: 100.0,
            ca_blue: -100.0,
            ..Lens::default()
        },
    );
    let moved = |ch: usize| {
        (0..240)
            .filter(|&x| fixed.get_pixel(x, 80).0[ch] != plain.get_pixel(x, 80).0[ch])
            .count()
    };
    assert!(moved(0) > 5, "red moved");
    assert!(moved(2) > 5, "blue moved");
    assert_eq!(moved(1), 0, "green stays");
}
