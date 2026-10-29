//! Point colour (desktop only): a picked colour's shifts change that colour and leave others.

use image::{Rgba, RgbaImage};
use lf_pipeline::Renderer;
use lf_pipeline::test_support::shared_gpu;
use lf_settings::EditSettings;
use lf_settings::point_colour::{Point, Points};

const BLUE: [u8; 3] = [40, 90, 210];
const RED: [u8; 3] = [200, 50, 40];

fn render(s: &EditSettings) -> RgbaImage {
    let img = RgbaImage::from_fn(64, 32, |x, _| {
        let c = if x < 32 { BLUE } else { RED };
        Rgba([c[0], c[1], c[2], 255])
    });
    let mut r = Renderer::new(shared_gpu(), &img, 64, 32).unwrap();
    r.render(s).unwrap()
}

fn with(points: Vec<Point>) -> EditSettings {
    let mut s = EditSettings::default_settings();
    Points(points).write_into(&mut s);
    s
}

fn chroma(p: Rgba<u8>) -> i32 {
    let [r, g, b, _] = p.0.map(i32::from);
    r.max(g).max(b) - r.min(g).min(b)
}

#[test]
fn a_picked_blue_is_desaturated_and_red_kept() {
    let plain = render(&EditSettings::default_settings());
    let picked = Point::picked(BLUE.map(|v| f64::from(v) / 255.0));
    let grey_blue = Point {
        saturation_shift: -100.0,
        ..picked
    };
    let out = render(&with(vec![grey_blue]));
    assert!(chroma(*plain.get_pixel(10, 10)) > 120);
    assert!(
        chroma(*out.get_pixel(10, 10)) < 15,
        "blue greyed: {:?}",
        out.get_pixel(10, 10)
    );
    assert_eq!(out.get_pixel(50, 10), plain.get_pixel(50, 10), "red kept");
    // A hue shift turns blue, not red.
    let turned = render(&with(vec![Point {
        hue_shift: 100.0,
        ..picked
    }]));
    assert_ne!(turned.get_pixel(10, 10), plain.get_pixel(10, 10));
    assert_eq!(turned.get_pixel(50, 10), plain.get_pixel(50, 10));
    // A point with no shift, or none at all: the web's pass exactly.
    assert_eq!(render(&with(vec![picked])), plain);
}
