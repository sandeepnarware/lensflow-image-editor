//! Colour calibration (desktop only): the primaries' hue and saturation move colours and
//! never greys; neutral is the web's pass exactly; the shadows tint colours the dark tones.

use image::{Rgba, RgbaImage};
use lf_pipeline::Renderer;
use lf_pipeline::test_support::shared_gpu;
use lf_settings::EditSettings;
use lf_settings::calibration::Calibration;

fn render(colour: [u8; 3], s: &EditSettings) -> [u8; 3] {
    let img = RgbaImage::from_pixel(64, 48, Rgba([colour[0], colour[1], colour[2], 255]));
    let mut r = Renderer::new(shared_gpu(), &img, 64, 48).unwrap();
    let out = r.render(s).unwrap();
    let p = out.get_pixel(32, 24).0;
    [p[0], p[1], p[2]]
}

fn with(c: Calibration) -> EditSettings {
    let mut s = EditSettings::default_settings();
    c.write_into(&mut s);
    s
}

#[test]
fn calibration_moves_colours_but_not_greys_and_tints_shadows() {
    let plain = EditSettings::default_settings();
    let red = [200, 60, 50];
    let before = render(red, &plain);
    // Neutral stores nothing and renders as the web does.
    assert_eq!(render(red, &with(Calibration::default())), before);

    let warm = with(Calibration {
        red_hue: 100.0,
        ..Calibration::default()
    });
    let after = render(red, &warm);
    assert!(
        i16::from(after[1]) > i16::from(before[1]) + 5,
        "red turns towards yellow: {before:?} -> {after:?}"
    );
    let grey = [128, 128, 128];
    let g = render(grey, &warm);
    assert!(
        g.iter().all(|v| (i16::from(*v) - 128).abs() <= 1),
        "grey stays grey: {g:?}"
    );

    let magenta_shadows = with(Calibration {
        shadows_tint: 100.0,
        ..Calibration::default()
    });
    let dark = render([40, 40, 40], &magenta_shadows);
    assert!(
        dark[0] > dark[1] && dark[2] > dark[1],
        "magenta in the shadows: {dark:?}"
    );
    let light = render([235, 235, 235], &magenta_shadows);
    assert!(
        light.iter().all(|v| (i16::from(*v) - 235).abs() <= 1),
        "highlights untouched: {light:?}"
    );
}
