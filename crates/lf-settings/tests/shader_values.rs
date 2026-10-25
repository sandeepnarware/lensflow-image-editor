use lf_settings::EditSettings;
use lf_settings::shader_values::{grade_offset, shader_values};

#[test]
fn defaults_are_neutral() {
    let v = shader_values(&EditSettings::default_settings());
    assert_eq!(v.wb, [1.0, 1.0, 1.0]);
    assert_eq!(v.grade_sh, [0.0; 3]);
    assert_eq!(v.hsl, [0.0; 24]);
}

#[test]
fn band_widths_are_the_larger_gap_to_a_neighbour() {
    assert_eq!(
        shader_values(&EditSettings::default_settings()).band_width,
        [45.0, 30.0, 60.0, 60.0, 60.0, 45.0, 45.0, 45.0]
    );
}

#[test]
fn warm_white_balance_keeps_white_as_bright() {
    let mut s = EditSettings::default_settings();
    s.temperature = 100.0;
    let wb = shader_values(&s).wb;
    let y = 0.2126 * wb[0] + 0.7152 * wb[1] + 0.0722 * wb[2];
    assert!((y - 1.0).abs() < 1e-5);
    assert!(wb[0] > wb[2]);
}

#[test]
fn grade_offset_has_zero_luma() {
    let o = grade_offset(190.0, 100.0);
    assert!((0.2126 * o[0] + 0.7152 * o[1] + 0.0722 * o[2]).abs() < 1e-6);
}
