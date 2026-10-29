//! The preview's focus peaking and visualize-spots overlays (desktop only): drawn over the
//! preview, never in a render or export.

use image::{Rgba, RgbaImage};
use lf_pipeline::Renderer;
use lf_pipeline::renderer::{DrawOptions, View};
use lf_pipeline::test_support::shared_gpu;
use lf_pipeline::texture::{Target, read_back};
use lf_settings::EditSettings;

fn draw(peaking: bool, spots_view: bool) -> RgbaImage {
    let gpu = shared_gpu();
    // A sharp edge down the middle: dark grey on the left, light grey on the right.
    let src = RgbaImage::from_fn(64, 48, |x, _| {
        if x < 32 {
            Rgba([60, 60, 60, 255])
        } else {
            Rgba([200, 200, 200, 255])
        }
    });
    let s = EditSettings::default_settings();
    let mut r = Renderer::new(gpu.clone(), &src, 64, 48).unwrap();
    let canvas = Target::new(&gpu, 64, 48, false).unwrap();
    let opts = DrawOptions {
        before: false,
        crop: None,
        view: View {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        },
        clipping: false,
        peaking,
        spots_view,
        proof: false,
    };
    r.draw(&s, &opts, &canvas).unwrap();
    read_back(&gpu, &canvas).unwrap()
}

#[test]
fn peaking_marks_sharp_edges_and_spots_view_shows_only_edges() {
    let plain = draw(false, false);
    let peak = draw(true, false);
    // The flat parts are as they were.
    assert_eq!(peak.get_pixel(8, 24), plain.get_pixel(8, 24));
    assert_eq!(peak.get_pixel(56, 24), plain.get_pixel(56, 24));
    // The edge is green.
    let edge = (30..34)
        .map(|x| peak.get_pixel(x, 24).0)
        .find(|p| p[1] > 200 && p[0] < 120)
        .is_some();
    assert!(
        edge,
        "no green at the edge: {:?}",
        (30..34)
            .map(|x| peak.get_pixel(x, 24).0)
            .collect::<Vec<_>>()
    );

    let spots = draw(false, true);
    assert!(spots.get_pixel(8, 24)[0] < 10, "flat is black");
    assert!(spots.get_pixel(56, 24)[0] < 10);
    assert!(
        (30..34).any(|x| spots.get_pixel(x, 24)[0] > 200),
        "the edge is white"
    );
}
