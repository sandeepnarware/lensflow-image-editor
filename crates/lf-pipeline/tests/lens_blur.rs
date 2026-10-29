//! Lens blur as an edit (desktop only): what is far from the focus blurs, what is in focus
//! stays sharp, and none at all is the plain render.

use image::{Rgba, RgbaImage};
use lf_pipeline::Renderer;
use lf_pipeline::test_support::shared_gpu;
use lf_settings::EditSettings;
use lf_settings::lens_blur::LensBlur;
use lf_settings::masks::encode_coverage;

const W: u32 = 240;
const H: u32 = 160;

/// Black and white stripes 4 px wide everywhere.
fn stripes() -> RgbaImage {
    RgbaImage::from_fn(W, H, |x, _| {
        if (x / 4) % 2 == 0 {
            Rgba([20, 20, 20, 255])
        } else {
            Rgba([235, 235, 235, 255])
        }
    })
}

fn render(s: &EditSettings) -> RgbaImage {
    let mut r = Renderer::new(shared_gpu(), &stripes(), W, H).unwrap();
    r.render(s).unwrap()
}

/// The left half near (in focus), the right half far.
fn with_blur(amount: f64) -> EditSettings {
    let (dw, dh) = (60u32, 40u32);
    let depth: Vec<u8> = (0..dw * dh)
        .map(|i| if i % dw < dw / 2 { 255 } else { 0 })
        .collect();
    let mut s = EditSettings::default_settings();
    LensBlur::write_into(
        Some(&LensBlur {
            amount,
            focus: 100.0,
            range: 10.0,
            width: dw,
            height: dh,
            depth: encode_coverage(&depth),
        }),
        &mut s,
    );
    s
}

/// How far the stripes' light and dark are apart along row `y` between `x0` and `x1`.
fn contrast(img: &RgbaImage, x0: u32, x1: u32, y: u32) -> i32 {
    let v: Vec<i32> = (x0..x1)
        .map(|x| i32::from(img.get_pixel(x, y).0[1]))
        .collect();
    v.iter().max().unwrap() - v.iter().min().unwrap()
}

#[test]
fn the_far_half_blurs_and_the_near_half_stays_sharp() {
    let plain = render(&EditSettings::default_settings());
    let blurred = render(&with_blur(100.0));
    assert!(contrast(&plain, 160, 220, 80) > 180);
    assert!(
        contrast(&blurred, 160, 220, 80) < 60,
        "far: {}",
        contrast(&blurred, 160, 220, 80)
    );
    assert!(
        contrast(&blurred, 20, 80, 80) > 180,
        "near: {}",
        contrast(&blurred, 20, 80, 80)
    );
    // None: the web's pass exactly.
    assert_eq!(render(&with_blur(0.0)), plain);
}
