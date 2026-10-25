//! Pixels for matching a reference: which photos stand for the selection and the contact
//! sheet they're tiled into (one mosaic serves both the AI request and the measurement loop,
//! so what the model saw is exactly what gets measured).

use image::imageops::{self, FilterType};
use image::{DynamicImage, RgbaImage};

/// Long edge of a bitmap used for measurement and preview.
pub const MEASURE_EDGE: u32 = 512;
/// Long edge of the pictures sent to the AI.
pub const AI_EDGE: u32 = 1024;
pub const TILE_EDGE: u32 = 256;
pub const SHEET_COLUMNS: u32 = 4;
pub const SHEET_ROWS: u32 = 6;
/// The largest JPEG sent to the AI.
pub const MAX_JPEG_BYTES: usize = 700 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum SheetError {
    #[error("the picture could not be encoded: {0}")]
    Encode(#[from] image::ImageError),
    #[error("the picture is still over 700 KB at the lowest quality")]
    TooLarge,
}

/// The photos whose pixels shape the grade: unedited ones (the grade is applied to the
/// untouched file), evenly spaced beyond `cap`. When every photo is edited, their current look
/// is used and the flag says so.
pub fn pick_samples<T>(items: &[T], is_edited: impl Fn(&T) -> bool, cap: usize) -> (Vec<&T>, bool) {
    if items.is_empty() {
        return (Vec::new(), false);
    }
    let unedited: Vec<&T> = items.iter().filter(|p| !is_edited(p)).collect();
    if unedited.is_empty() {
        (spaced(items.iter().collect(), cap), true)
    } else {
        (spaced(unedited, cap), false)
    }
}

/// Up to `cap` items spread evenly through the list, first and last included.
fn spaced<T>(items: Vec<T>, cap: usize) -> Vec<T> {
    if items.len() <= cap {
        return items;
    }
    if cap <= 1 {
        return items.into_iter().take(1).collect();
    }
    let last = items.len() - 1;
    let picks: Vec<usize> = (0..cap)
        .map(|i| ((i * last) as f64 / (cap - 1) as f64 + 0.5).floor() as usize)
        .collect();
    let mut slots: Vec<Option<T>> = items.into_iter().map(Some).collect();
    // Picks are strictly increasing when cap < len, so each item is taken at most once.
    picks.into_iter().filter_map(|i| slots[i].take()).collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub w: u32,
    pub h: u32,
    pub rows: u32,
    /// The [x, y] of each tile.
    pub rects: Vec<(u32, u32)>,
}

/// Square tiles in rows of `columns`, at most `SHEET_ROWS` rows.
pub fn tile_layout(count: usize, tile_edge: u32, columns: u32) -> Layout {
    let n = count.min((columns * SHEET_ROWS) as usize) as u32;
    let rows = n.div_ceil(columns).max(1);
    Layout {
        w: n.min(columns) * tile_edge,
        h: rows * tile_edge,
        rows,
        rects: (0..n)
            .map(|i| ((i % columns) * tile_edge, (i / columns) * tile_edge))
            .collect(),
    }
}

/// The sampled photos tiled into one image, each cover-fitted into its square.
pub fn tile(bitmaps: &[RgbaImage], tile_edge: u32, columns: u32) -> RgbaImage {
    let layout = tile_layout(bitmaps.len(), tile_edge, columns);
    let mut sheet = RgbaImage::new(layout.w.max(1), layout.h.max(1));
    for ((x, y), bmp) in layout.rects.iter().zip(bitmaps) {
        let (bw, bh) = (bmp.width().max(1) as f64, bmp.height().max(1) as f64);
        let k = (tile_edge as f64 / bw).max(tile_edge as f64 / bh);
        let w = ((bw * k).round() as u32).max(tile_edge);
        let h = ((bh * k).round() as u32).max(tile_edge);
        let fitted = imageops::resize(bmp, w, h, FilterType::Triangle);
        let cropped = imageops::crop_imm(
            &fitted,
            (w - tile_edge) / 2,
            (h - tile_edge) / 2,
            tile_edge,
            tile_edge,
        );
        imageops::replace(&mut sheet, &*cropped, *x as i64, *y as i64);
    }
    sheet
}

/// The image scaled down so its long edge is `long_edge` (never enlarged).
pub fn scaled(img: &RgbaImage, long_edge: u32) -> RgbaImage {
    let long = img.width().max(img.height());
    if long <= long_edge {
        return img.clone();
    }
    let k = long_edge as f64 / long as f64;
    let w = ((img.width() as f64 * k).round() as u32).max(1);
    let h = ((img.height() as f64 * k).round() as u32).max(1);
    imageops::resize(img, w, h, FilterType::Triangle)
}

/// A JPEG of the image at `long_edge`: starts at `quality` (0..1, the web's 0.85), steps down
/// by 0.05 until it is at most 700 KB, never below 0.6.
pub fn jpeg(img: &RgbaImage, long_edge: u32, quality: f64) -> Result<Vec<u8>, SheetError> {
    let rgb = DynamicImage::ImageRgba8(scaled(img, long_edge)).to_rgb8();
    let mut q = (quality * 100.0).round() as i32;
    loop {
        let mut out = Vec::new();
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, q as u8);
        rgb.write_with_encoder(encoder)?;
        if out.len() <= MAX_JPEG_BYTES {
            return Ok(out);
        }
        if q - 5 < 60 {
            return Err(SheetError::TooLarge);
        }
        q -= 5;
    }
}
