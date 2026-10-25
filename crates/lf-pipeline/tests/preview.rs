//! Preview drawing, histogram and pixels (the web's `draw`, `histogram` and `pixels`).

use image::{Rgba, RgbaImage};
use lf_fixtures::{case_json, load_png, root};
use lf_pipeline::renderer::{DrawOptions, View};
use lf_pipeline::texture::{Target, read_back};
use lf_pipeline::{Gpu, Renderer};
use lf_settings::normalise;
use serde_json::json;

fn source(name: &str) -> RgbaImage {
    load_png(root().join(format!("sources/{name}.png"))).unwrap()
}

#[test]
fn full_view_draw_equals_render() {
    let gpu = Gpu::new().unwrap();
    let src = source("portrait");
    let s = normalise(&case_json("look-golden").unwrap());
    let mut r = Renderer::new(&gpu, &src, src.width(), src.height()).unwrap();
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
    };
    r.draw(&s, &opts, &canvas).unwrap();
    let got = read_back(&gpu, &canvas).unwrap();
    assert_eq!(got.dimensions(), want.dimensions());
    assert!(got.as_raw() == want.as_raw(), "draw differs from render");
}

#[test]
fn before_ignores_settings_and_geometry() {
    let gpu = Gpu::new().unwrap();
    let src = source("portrait");
    let s = normalise(&json!({ "rotate": 90, "exposure": 2 }));
    let mut r = Renderer::new(&gpu, &src, src.width(), src.height()).unwrap();
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
fn histogram_counts_every_pixel() {
    let gpu = Gpu::new().unwrap();
    let src = source("landscape");
    let s = normalise(&json!({}));
    let mut r = Renderer::new(&gpu, &src, src.width(), src.height()).unwrap();
    let h = r.histogram(&s).unwrap();
    let (w, hh) = r.pixels(&s, 200).unwrap().dimensions();
    let n = u64::from(w) * u64::from(hh);
    for bins in [&h.r, &h.g, &h.b, &h.l] {
        assert_eq!(bins.iter().map(|&c| u64::from(c)).sum::<u64>(), n);
    }

    let grey = RgbaImage::from_pixel(64, 64, Rgba([128, 128, 128, 255]));
    let mut r = Renderer::new(&gpu, &grey, 64, 64).unwrap();
    let h = r.histogram(&s).unwrap();
    assert_eq!(h.l[128], 200 * 200);
    assert_eq!(h.l.iter().filter(|&&c| c > 0).count(), 1);
}

#[test]
fn pixels_scales_to_the_long_edge() {
    let gpu = Gpu::new().unwrap();
    let src = source("landscape");
    assert_eq!(src.dimensions(), (512, 341));
    let s = normalise(&json!({}));
    let mut r = Renderer::new(&gpu, &src, src.width(), src.height()).unwrap();
    assert_eq!(r.pixels(&s, 100).unwrap().dimensions(), (100, 67));
}
