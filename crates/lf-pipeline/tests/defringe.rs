//! Defringe (desktop only): purple next to a strong edge is greyed; purple away from edges,
//! and everything with defringe off, is left as it was.

use image::{Rgba, RgbaImage};
use lf_pipeline::Renderer;
use lf_pipeline::test_support::shared_gpu;
use lf_settings::EditSettings;
use lf_settings::defringe::Defringe;

const PURPLE: Rgba<u8> = Rgba([150, 60, 200, 255]);

/// Dark on the left, white on the right with a purple fringe 3 pixels wide on the edge, and
/// a large purple patch far from any edge in the dark part.
fn scene() -> RgbaImage {
    RgbaImage::from_fn(120, 80, |x, y| {
        if (60..63).contains(&x) {
            PURPLE
        } else if x >= 63 {
            Rgba([250, 250, 250, 255])
        } else if (10..40).contains(&x) && (20..60).contains(&y) {
            PURPLE
        } else {
            Rgba([30, 30, 30, 255])
        }
    })
}

fn render(s: &EditSettings) -> RgbaImage {
    let img = scene();
    let mut r = Renderer::new(shared_gpu(), &img, 120, 80).unwrap();
    r.render(s).unwrap()
}

fn chroma(p: Rgba<u8>) -> i32 {
    let [r, g, b, _] = p.0.map(i32::from);
    r.max(g).max(b) - r.min(g).min(b)
}

#[test]
fn a_fringe_on_an_edge_is_greyed_and_purple_elsewhere_kept() {
    let plain = render(&EditSettings::default_settings());
    let mut s = EditSettings::default_settings();
    Defringe {
        purple_amount: 20.0,
        ..Defringe::default()
    }
    .write_into(&mut s);
    let fixed = render(&s);
    let fringe = (61, 40);
    assert!(chroma(*plain.get_pixel(fringe.0, fringe.1)) > 80);
    assert!(
        chroma(*fixed.get_pixel(fringe.0, fringe.1)) < 25,
        "the fringe: {:?}",
        fixed.get_pixel(fringe.0, fringe.1)
    );
    assert_eq!(
        fixed.get_pixel(25, 40),
        plain.get_pixel(25, 40),
        "purple far from an edge"
    );
    // Off: the web's pass exactly.
    let mut off = EditSettings::default_settings();
    Defringe::default().write_into(&mut off);
    assert_eq!(render(&off), plain);
    // A green amount alone leaves purple.
    let mut green = EditSettings::default_settings();
    Defringe {
        green_amount: 20.0,
        ..Defringe::default()
    }
    .write_into(&mut green);
    assert!(chroma(*render(&green).get_pixel(fringe.0, fringe.1)) > 80);
}
