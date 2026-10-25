//! Contact sheet sampling and layout, ported from the web's `contactSheet.spec.ts`, plus the
//! tiling and JPEG steps the browser did with canvas.

use image::{Rgba, RgbaImage};
use lf_grade::sheet::{self, Layout, pick_samples, tile_layout};

struct Photo {
    id: u32,
    is_edited: bool,
}

fn photos(n: u32, edited: impl Fn(u32) -> bool) -> Vec<Photo> {
    (0..n)
        .map(|i| Photo {
            id: i + 1,
            is_edited: edited(i),
        })
        .collect()
}

fn ids(samples: &[&Photo]) -> Vec<u32> {
    samples.iter().map(|p| p.id).collect()
}

#[test]
fn takes_unedited_photos_only_evenly_spaced_beyond_the_cap() {
    let all = photos(60, |_| false);
    let (samples, all_edited) = pick_samples(&all, |p| p.is_edited, 24);
    assert_eq!(samples.len(), 24);
    assert!(!all_edited);
    assert_eq!(samples[0].id, 1);
    assert_eq!(samples[23].id, 60);
    // Spacing is even: consecutive gaps differ by at most one.
    let gaps: Vec<u32> = samples.windows(2).map(|w| w[1].id - w[0].id).collect();
    assert!(gaps.iter().max().unwrap() - gaps.iter().min().unwrap() <= 1);
}

#[test]
fn leaves_edited_photos_out_of_the_baseline_while_any_unedited_remain() {
    let all = photos(10, |i| i % 2 == 0);
    let (samples, all_edited) = pick_samples(&all, |p| p.is_edited, 24);
    assert_eq!(ids(&samples), [2, 4, 6, 8, 10]);
    assert!(!all_edited);
}

#[test]
fn falls_back_to_the_edited_photos_and_says_so_when_nothing_is_unedited() {
    let all = photos(30, |_| true);
    let (samples, all_edited) = pick_samples(&all, |p| p.is_edited, 12);
    assert_eq!(samples.len(), 12);
    assert!(all_edited);
    let none: Vec<Photo> = Vec::new();
    let (samples, all_edited) = pick_samples(&none, |p| p.is_edited, 24);
    assert!(samples.is_empty());
    assert!(!all_edited);
}

#[test]
fn tiles_squares_in_rows_of_four_at_most_six_rows() {
    assert_eq!(
        tile_layout(5, 256, 4),
        Layout {
            w: 1024,
            h: 512,
            rows: 2,
            rects: vec![(0, 0), (256, 0), (512, 0), (768, 0), (0, 256)]
        }
    );
    let one = tile_layout(1, 256, 4);
    assert_eq!((one.w, one.h, one.rows), (256, 256, 1));
    let full = tile_layout(24, 256, 4);
    assert_eq!((full.w, full.h, full.rows), (1024, 1536, 6));
    assert_eq!(tile_layout(30, 256, 4).rects.len(), 24);
}

#[test]
fn cover_fits_each_bitmap_into_its_square() {
    let wide = RgbaImage::from_pixel(400, 100, Rgba([200, 0, 0, 255]));
    let tall = RgbaImage::from_pixel(100, 400, Rgba([0, 0, 200, 255]));
    let sheet = sheet::tile(&[wide, tall], 64, 4);
    assert_eq!((sheet.width(), sheet.height()), (128, 64));
    assert_eq!(sheet.get_pixel(0, 0), &Rgba([200, 0, 0, 255]));
    assert_eq!(sheet.get_pixel(63, 63), &Rgba([200, 0, 0, 255]));
    assert_eq!(sheet.get_pixel(64, 0), &Rgba([0, 0, 200, 255]));
    assert_eq!(sheet.get_pixel(127, 63), &Rgba([0, 0, 200, 255]));
}

#[test]
fn scales_to_the_long_edge_and_never_enlarges() {
    let img = RgbaImage::new(400, 200);
    let s = sheet::scaled(&img, 100);
    assert_eq!((s.width(), s.height()), (100, 50));
    let same = sheet::scaled(&img, 1000);
    assert_eq!((same.width(), same.height()), (400, 200));
}

#[test]
fn encodes_a_jpeg_within_700_kb() {
    // Noise is the worst case for a JPEG; a 1024 px sheet of it still has to fit or error.
    let mut seed = 1u32;
    let img = RgbaImage::from_fn(1024, 1024, |_, _| {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        let b = (seed >> 24) as u8;
        Rgba([b, b.wrapping_mul(3), b.wrapping_add(90), 255])
    });
    match sheet::jpeg(&img, 1024, 0.85) {
        Ok(bytes) => {
            assert!(bytes.len() <= 700 * 1024);
            assert_eq!(&bytes[..2], [0xFF, 0xD8]);
        }
        Err(e) => assert!(matches!(e, sheet::SheetError::TooLarge)),
    }
    let smooth = RgbaImage::from_pixel(512, 512, Rgba([120, 130, 140, 255]));
    let bytes = sheet::jpeg(&smooth, 256, 0.85).unwrap();
    assert!(bytes.len() < 700 * 1024);
}
