//! Preview drawing, histogram and pixels (the web's `draw`, `histogram` and `pixels`).

use image::{Rgba, RgbaImage};
use lf_fixtures::{case_json, load_png, root};
use lf_pipeline::Renderer;
use lf_pipeline::renderer::{DrawOptions, View};
use lf_pipeline::test_support::shared_gpu;
use lf_pipeline::texture::{Target, read_back};
use lf_settings::normalise;
use serde_json::json;

fn source(name: &str) -> RgbaImage {
    load_png(root().join(format!("sources/{name}.png"))).unwrap()
}

#[test]
fn full_view_draw_equals_render() {
    let gpu = shared_gpu();
    let src = source("portrait");
    let s = normalise(&case_json("look-golden").unwrap());
    let mut r = Renderer::new(gpu.clone(), &src, src.width(), src.height()).unwrap();
    let want = r.render(&s).unwrap();
    let canvas = Target::new(&gpu, want.width(), want.height(), false).unwrap();
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
    };
    r.draw(&s, &opts, &canvas).unwrap();
    let got = read_back(&gpu, &canvas).unwrap();
    assert_eq!(got.dimensions(), want.dimensions());
    assert!(got.as_raw() == want.as_raw(), "draw differs from render");
}

#[test]
fn before_ignores_settings_and_geometry() {
    let gpu = shared_gpu();
    let src = source("portrait");
    let s = normalise(&json!({ "rotate": 90, "exposure": 2 }));
    let mut r = Renderer::new(gpu.clone(), &src, src.width(), src.height()).unwrap();
    let canvas = Target::new(&gpu, src.width(), src.height(), false).unwrap();
    let opts = DrawOptions {
        before: true,
        ..DrawOptions::default()
    };
    r.draw(&s, &opts, &canvas).unwrap();
    let got = read_back(&gpu, &canvas).unwrap();
    assert!(got.as_raw() == src.as_raw(), "before is not the source");
}

#[test]
fn clipping_shows_white_red_and_black_blue_only_when_asked() {
    let gpu = shared_gpu();
    // Left half black, right half white, a grey stripe in the middle.
    let src = RgbaImage::from_fn(32, 8, |x, _| match x {
        0..=13 => Rgba([0, 0, 0, 255]),
        14..=17 => Rgba([128, 128, 128, 255]),
        _ => Rgba([255, 255, 255, 255]),
    });
    let s = normalise(&json!({}));
    let mut r = Renderer::new(gpu.clone(), &src, 32, 8).unwrap();
    let canvas = Target::new(&gpu, 32, 8, false).unwrap();
    let opts = DrawOptions {
        clipping: true,
        ..DrawOptions::default()
    };
    r.draw(&s, &opts, &canvas).unwrap();
    let got = read_back(&gpu, &canvas).unwrap();
    let px = |x| got.get_pixel(x, 4).0;
    assert!(
        px(2)[2] > 200 && px(2)[0] < 100,
        "black is not blue: {:?}",
        px(2)
    );
    assert!(
        px(29)[0] > 200 && px(29)[1] < 100,
        "white is not red: {:?}",
        px(29)
    );
    assert_eq!(px(16)[0], px(16)[2], "grey changed");
    // Off: the draw is the render.
    r.draw(&s, &DrawOptions::default(), &canvas).unwrap();
    let plain = read_back(&gpu, &canvas).unwrap();
    assert_eq!(plain.get_pixel(29, 4).0, [255, 255, 255, 255]);
}

#[test]
fn histogram_counts_every_pixel() {
    let gpu = shared_gpu();
    let src = source("landscape");
    let s = normalise(&json!({}));
    let mut r = Renderer::new(gpu.clone(), &src, src.width(), src.height()).unwrap();
    let h = r.histogram(&s).unwrap();
    let (w, hh) = r.pixels(&s, 200).unwrap().dimensions();
    let n = u64::from(w) * u64::from(hh);
    for bins in [&h.r, &h.g, &h.b, &h.l] {
        assert_eq!(bins.iter().map(|&c| u64::from(c)).sum::<u64>(), n);
    }

    let grey = RgbaImage::from_pixel(64, 64, Rgba([128, 128, 128, 255]));
    let mut r = Renderer::new(gpu.clone(), &grey, 64, 64).unwrap();
    let h = r.histogram(&s).unwrap();
    assert_eq!(h.l[128], 200 * 200);
    assert_eq!(h.l.iter().filter(|&&c| c > 0).count(), 1);
}

#[test]
fn pixels_scales_to_the_long_edge() {
    let gpu = shared_gpu();
    let src = source("landscape");
    assert_eq!(src.dimensions(), (512, 341));
    let s = normalise(&json!({}));
    let mut r = Renderer::new(gpu.clone(), &src, src.width(), src.height()).unwrap();
    assert_eq!(r.pixels(&s, 100).unwrap().dimensions(), (100, 67));
}

/// A minified draw (preview, thumbnail, histogram) goes through the mip chain in every
/// `texture()` of the develop pass, the noise loop's included. This checks the cheap part of
/// that invariant: a half-size draw of every case (one source) succeeds, fills the canvas at
/// the size asked for and draws the photo there (not the background), on drivers whose
/// helper lanes past the edge are garbage (WARP) as well.
#[test]
fn minified_draw_succeeds_for_every_case() {
    let gpu = shared_gpu();
    let src = source("landscape");
    let m = lf_fixtures::Manifest::load().unwrap();
    let mut r = Renderer::new(gpu.clone(), &src, src.width(), src.height()).unwrap();
    for id in &m.cases {
        let s = normalise(&case_json(id).unwrap());
        let (ow, oh) = r.output_size(&s);
        let (w, h) = ((ow / 2).max(1), (oh / 2).max(1));
        let canvas = Target::new(&gpu, w, h, false).unwrap();
        r.draw(&s, &DrawOptions::default(), &canvas).unwrap();
        let got = read_back(&gpu, &canvas).unwrap();
        assert_eq!(got.dimensions(), (w, h), "{id}");
        let centre = got.get_pixel(w / 2, h / 2).0;
        assert_ne!(
            centre,
            [31, 31, 33, 255],
            "{id}: the centre is the background"
        );
    }
}

#[test]
fn renderer_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<Renderer>();
    assert_send::<std::sync::Arc<lf_pipeline::Gpu>>();
}

#[test]
fn histogram_keeps_the_thumbnail_and_trim_drops_the_outputs() {
    let gpu = shared_gpu();
    let src = source("landscape");
    let s = normalise(&json!({ "exposure": 0.2 }));
    let mut r = Renderer::new(gpu.clone(), &src, src.width(), src.height()).unwrap();
    let thumb = r.pixels(&s, 120).unwrap();
    let _ = r.histogram(&s).unwrap();
    let _ = r.render(&s).unwrap();
    for name in ["output", "thumb", "hist"] {
        assert!(r.has_cached(name), "{name} missing before trim");
    }
    r.trim();
    for name in ["output", "thumb", "hist"] {
        assert!(!r.has_cached(name), "{name} kept after trim");
    }
    // Everything still works after a trim, and draws the same.
    assert_eq!(r.pixels(&s, 120).unwrap(), thumb);
    let h = r.histogram(&s).unwrap();
    assert!(h.l.iter().sum::<u32>() > 0);
}

#[test]
fn many_draws_reuse_the_develop_uniforms() {
    // Settings that change between draws must each reach their own draw.
    let gpu = shared_gpu();
    let src = source("portrait");
    let mut r = Renderer::new(gpu.clone(), &src, src.width(), src.height()).unwrap();
    let dark = normalise(&json!({ "exposure": -2 }));
    let bright = normalise(&json!({ "exposure": 2 }));
    let want_dark = r.pixels(&dark, 64).unwrap();
    let want_bright = r.pixels(&bright, 64).unwrap();
    assert_ne!(want_dark, want_bright);
    for _ in 0..3 {
        assert_eq!(r.pixels(&dark, 64).unwrap(), want_dark);
        assert_eq!(r.pixels(&bright, 64).unwrap(), want_bright);
    }
}
