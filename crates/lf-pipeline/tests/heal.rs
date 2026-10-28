//! Desktop-only healing strokes: a brushed line healed from beside it, through the spot pass
//! in chains of dabs (more than one pass of 48 for a long stroke).

use image::{Rgba, RgbaImage};
use lf_pipeline::Renderer;
use lf_pipeline::test_support::shared_gpu;
use lf_settings::heal::{HealStroke, Heals};
use lf_settings::{EditSettings, SpotMode};

#[test]
fn a_long_stroke_heals_a_dark_line_away() {
    let (w, h) = (320u32, 200u32);
    // Flat grey with a dark horizontal line across most of the width.
    let img = RgbaImage::from_fn(w, h, |x, y| {
        if (98..=101).contains(&y) && (20..300).contains(&x) {
            Rgba([20, 20, 20, 255])
        } else {
            Rgba([150, 150, 150, 255])
        }
    });
    let mut r = Renderer::new(shared_gpu(), &img, w, h).unwrap();
    let plain = r.render(&EditSettings::default_settings()).unwrap();
    assert!(plain.get_pixel(160, 100).0[0] < 60);
    let mut s = EditSettings::default_settings();
    let stroke = HealStroke {
        points: vec![[15.0 / 320.0, 0.5], [305.0 / 320.0, 0.5]],
        r: 0.02,
        dx: 0.0,
        dy: 0.15,
        mode: SpotMode::Heal,
    };
    // Long enough for several passes of 48 dabs.
    assert!(lf_settings::heal::dabs(&stroke, 1.6).len() > 100);
    Heals(vec![stroke]).write_into(&mut s);
    let healed = r.render(&s).unwrap();
    for x in [40, 160, 280] {
        let v = healed.get_pixel(x, 100).0[0];
        assert!(v > 130, "the line still shows at x {x}: {v}");
    }
    // Away from the stroke nothing changed.
    assert_eq!(healed.get_pixel(160, 20), plain.get_pixel(160, 20));
}
