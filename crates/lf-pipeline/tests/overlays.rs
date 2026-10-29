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

/// Side by side: the photo as it was and the edit drawn into two canvases from one renderer,
/// one after the other, each as it is when drawn alone.
#[test]
fn before_and_after_canvases_do_not_disturb_each_other() {
    let gpu = shared_gpu();
    let src = RgbaImage::from_fn(64, 48, |x, y| Rgba([(x * 3) as u8, (y * 4) as u8, 90, 255]));
    let mut s = EditSettings::default_settings();
    s.exposure = 1.0;
    let mut r = Renderer::new(gpu.clone(), &src, 64, 48).unwrap();
    let whole = View {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
    };
    let after_opts = DrawOptions {
        view: whole,
        ..DrawOptions::default()
    };
    let before_opts = DrawOptions {
        before: true,
        view: whole,
        ..DrawOptions::default()
    };
    let (a, b) = (
        Target::new(&gpu, 64, 48, false).unwrap(),
        Target::new(&gpu, 64, 48, false).unwrap(),
    );
    r.draw(&s, &after_opts, &a).unwrap();
    let alone = read_back(&gpu, &a).unwrap();
    r.draw(&s, &before_opts, &b).unwrap();
    r.draw(&s, &after_opts, &a).unwrap();
    assert_eq!(read_back(&gpu, &a).unwrap(), alone, "the edit is unchanged");
    let before = read_back(&gpu, &b).unwrap();
    assert_eq!(before.get_pixel(10, 10).0[2], 90, "the photo as it was");
    assert!(alone.get_pixel(10, 10).0[2] > 120, "the edit is brighter");
}
