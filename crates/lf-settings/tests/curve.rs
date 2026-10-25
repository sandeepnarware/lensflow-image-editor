use lf_settings::Curves;
use lf_settings::curve::{curve_table, pack_curves};

#[test]
fn identity_curve_is_a_ramp() {
    let t = curve_table(&[[0.0, 0.0], [1.0, 1.0]], 256);
    for (i, v) in t.iter().enumerate() {
        assert!((v - i as f32 / 255.0).abs() < 1e-6);
    }
}

#[test]
fn fewer_than_two_points_is_a_ramp() {
    assert_eq!(curve_table(&[[0.3, 0.9]], 256), curve_table(&[], 256));
}

#[test]
fn curve_passes_through_its_points_and_never_overshoots() {
    let pts = [[0.0, 0.0], [0.25, 0.1], [0.5, 0.6], [0.75, 0.7], [1.0, 1.0]];
    let t = curve_table(&pts, 256);
    for [x, y] in pts {
        assert!((t[(x * 255.0_f64).round() as usize] - y as f32).abs() < 0.01);
    }
    assert!(t.windows(2).all(|w| w[1] >= w[0] - 1e-6));
}

#[test]
fn pack_curves_identity_is_the_diagonal() {
    let p = pack_curves(&Curves::identity());
    for i in 0..256 {
        assert_eq!(&p[i * 4..i * 4 + 4], &[i as u8, i as u8, i as u8, 255]);
    }
}
