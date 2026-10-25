use lf_settings::geometry::{
    Geometry, apply, crop_inside, fit_crop, largest_crop, output_size, output_to_source,
    turned_size,
};
use lf_settings::{Crop, EditSettings, Rotate};

fn assert_close(got: (f64, f64), want: (f64, f64)) {
    assert!(
        (got.0 - want.0).abs() < 1e-9 && (got.1 - want.1).abs() < 1e-9,
        "{got:?} != {want:?}"
    );
}

#[test]
fn quarter_turn_maps_output_corners_to_source() {
    let g = Geometry {
        rotate: Rotate::R90,
        flip_h: false,
        flip_v: false,
        angle: 0.0,
        crop: Crop {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        },
    };
    let m = output_to_source(400, 300, &g, &g.crop);
    assert_close(apply(&m, 0.0, 0.0), (0.0, 1.0));
    assert_close(apply(&m, 1.0, 0.0), (0.0, 0.0));
    assert_close(apply(&m, 1.0, 1.0), (1.0, 0.0));
    assert_eq!(turned_size(400, 300, Rotate::R90), (300, 400));
    assert_eq!(output_size(400, 300, &g), (300, 400));
}

#[test]
fn identity_geometry_is_the_identity_map() {
    let g = Geometry::from(&EditSettings::default_settings());
    let m = output_to_source(640, 480, &g, &g.crop);
    assert_close(apply(&m, 0.25, 0.75), (0.25, 0.75));
}

#[test]
fn crop_fits_after_straightening() {
    let c = largest_crop(1000.0, 1000.0, 45.0, Some(1.0));
    assert!((c.w - c.h).abs() < 1e-12);
    assert!((c.x - (1.0 - c.w) / 2.0).abs() < 1e-12);
    assert!(crop_inside(&c, 1000.0, 1000.0, 45.0));

    let full = Crop {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
    };
    let fitted = fit_crop(&full, 1000.0, 1000.0, 10.0);
    assert!(fitted.w < 1.0 && fitted.h < 1.0);
    assert!(crop_inside(&fitted, 1000.0, 1000.0, 10.0));
}
