//! 3D LUT looks (desktop only): the develop pass looks the finished colour up in the chosen
//! table, mixed by its amount; a LUT that is not loaded changes nothing.

use image::{Rgba, RgbaImage};
use lf_pipeline::Renderer;
use lf_pipeline::test_support::shared_gpu;
use lf_settings::EditSettings;
use lf_settings::lut::{Lut, LutChoice};

/// The registry is process-wide and one test clears it: they take turns.
static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn render(s: &EditSettings) -> [u8; 3] {
    let img = RgbaImage::from_pixel(64, 48, Rgba([200, 100, 50, 255]));
    let mut r = Renderer::new(shared_gpu(), &img, 64, 48).unwrap();
    let out = r.render(s).unwrap();
    let p = out.get_pixel(32, 24).0;
    [p[0], p[1], p[2]]
}

fn with(name: &str, amount: f64) -> EditSettings {
    let mut s = EditSettings::default_settings();
    LutChoice::write_into(
        Some(&LutChoice {
            name: name.into(),
            amount,
        }),
        &mut s,
    );
    s
}

fn near(got: [u8; 3], want: [u8; 3]) -> bool {
    got.iter()
        .zip(want)
        .all(|(g, w)| (i16::from(*g) - i16::from(w)).abs() <= 2)
}

#[test]
fn a_lut_changes_the_colour_by_its_amount_and_a_missing_one_does_nothing() {
    let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let mut invert = Lut::identity(17);
    for p in &mut invert.data {
        *p = p.map(|v| 1.0 - v);
    }
    lf_pipeline::lut::register("test-invert", invert);
    assert!(lf_pipeline::lut::names().contains(&"test-invert".to_owned()));

    let plain = render(&EditSettings::default_settings());
    assert!(near(plain, [200, 100, 50]), "{plain:?}");
    let full = render(&with("test-invert", 100.0));
    assert!(near(full, [55, 155, 205]), "{full:?}");
    let half = render(&with("test-invert", 50.0));
    assert!(near(half, [128, 128, 128]), "{half:?}");
    assert_eq!(render(&with("not-loaded", 100.0)), plain);
}

#[test]
fn a_pinned_table_outlives_a_clear_until_unpinned() {
    let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let pinned = lf_pipeline::lut::pin("test-pinned", Lut::identity(2));
    lf_pipeline::lut::clear();
    assert!(
        lf_pipeline::lut::get("test-pinned").is_some(),
        "a catalogue opening meanwhile keeps it"
    );
    drop(pinned);
    assert!(
        lf_pipeline::lut::get("test-pinned").is_some(),
        "unpinning leaves it registered"
    );
    lf_pipeline::lut::clear();
    assert!(lf_pipeline::lut::get("test-pinned").is_none());
}

#[test]
fn forgetting_a_new_pin_unregisters_it_but_not_an_old_name() {
    let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    lf_pipeline::lut::clear();
    lf_pipeline::lut::pin("test-fresh", Lut::identity(2)).forget();
    assert!(
        lf_pipeline::lut::get("test-fresh").is_none(),
        "a table only the fit needed is not left on offer"
    );
    lf_pipeline::lut::register("test-old", Lut::identity(2));
    lf_pipeline::lut::pin("test-old", Lut::identity(2)).forget();
    assert!(
        lf_pipeline::lut::get("test-old").is_some(),
        "a name registered before stays"
    );
    // Held by another guard: stays until the last one is gone.
    let first = lf_pipeline::lut::pin("test-shared", Lut::identity(2));
    lf_pipeline::lut::pin("test-shared", Lut::identity(2)).forget();
    assert!(lf_pipeline::lut::get("test-shared").is_some());
    drop(first);
    lf_pipeline::lut::clear();
}
