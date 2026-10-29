//! Film negative conversion (desktop only): a negative's pixels become their positive in the
//! develop pass, first, so the other sliders work on the positive; none is the web's pass.

use image::{Rgba, RgbaImage};
use lf_pipeline::Renderer;
use lf_pipeline::test_support::shared_gpu;
use lf_settings::EditSettings;
use lf_settings::negative::Negative;

fn render(px: [u8; 3], s: &EditSettings) -> [u8; 3] {
    let img = RgbaImage::from_pixel(32, 24, Rgba([px[0], px[1], px[2], 255]));
    let mut r = Renderer::new(shared_gpu(), &img, 32, 24).unwrap();
    let out = r.render(s).unwrap();
    let p = out.get_pixel(16, 12).0;
    [p[0], p[1], p[2]]
}

#[test]
fn a_negative_renders_as_its_positive() {
    // An orange film base: unexposed film (the base itself) is black in the positive; dense
    // film (much less light through) is bright.
    let base = [0.8f64, 0.45, 0.25];
    let n = Negative {
        base,
        dmax: [2.0, 2.0, 2.0],
    };
    let mut s = EditSettings::default_settings();
    Negative::write_into(Some(&n), &mut s);
    let enc = |lin: f64| ((lin.powf(1.0 / 2.2)) * 255.0).round() as u8;
    let film = [enc(base[0]), enc(base[1]), enc(base[2])];
    let black = render(film, &s);
    assert!(black.iter().all(|v| *v < 12), "base is black: {black:?}");
    // One density unit (a tenth of the light) on every channel: the same grey in each.
    let dense = [
        enc(base[0] / 10.0),
        enc(base[1] / 10.0),
        enc(base[2] / 10.0),
    ];
    let grey = render(dense, &s);
    assert!(
        grey[0].abs_diff(grey[1]) <= 6 && grey[1].abs_diff(grey[2]) <= 6,
        "{grey:?}"
    );
    assert!(grey[0] > 120, "{grey:?}");
    // No conversion: the web's pass.
    let plain = EditSettings::default_settings();
    assert_eq!(render(film, &plain), film);
}
