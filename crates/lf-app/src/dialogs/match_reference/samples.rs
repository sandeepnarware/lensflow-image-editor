//! The photos that stand for the selection, and the contact sheet they are tiled into: up to
//! 24 of them (unedited ones first, as the web), each rendered with default settings, since
//! the grade is fitted to the untouched files.

use image::RgbaImage;
use lf_catalog::PhotoRow;
use lf_grade::sheet::{SHEET_COLUMNS, TILE_EDGE, pick_samples, tile};
use lf_grade::{Stats, measure};
use std::collections::HashMap;
use std::path::Path;

/// The most photos read for one grade.
pub const CAP: usize = 24;
pub const NONE_READ: &str = "None of the selected photos could be read.";

/// The sampled photos' pictures, in order.
#[derive(Debug, Clone)]
pub struct Sheet {
    pub ids: Vec<i64>,
    pub bitmaps: Vec<RgbaImage>,
    /// Every selected photo already has an edit (their files are still read unedited).
    pub all_edited: bool,
    /// Photos that could not be read and were left out.
    pub skipped: usize,
}

impl Sheet {
    /// The contact sheet: the bitmaps tiled in rows of four.
    pub fn tiled(&self) -> RgbaImage {
        tile(&self.bitmaps, TILE_EDGE, SHEET_COLUMNS)
    }

    /// Each sampled photo's median luma (for matching exposure per photo).
    pub fn medians(&self) -> HashMap<i64, f64> {
        self.ids
            .iter()
            .zip(&self.bitmaps)
            .map(|(id, b)| (*id, median(b)))
            .collect()
    }
}

/// The picture's measured statistics (every pixel).
pub fn stats(img: &RgbaImage) -> Stats {
    measure(img.as_raw(), 1)
}

/// The picture's median luma.
pub fn median(img: &RgbaImage) -> f64 {
    stats(img).median
}

/// Picks the samples (`pick_samples`, at most [`CAP`], unedited first) and reads each with
/// `open` (the file rendered with default settings). A photo that cannot be read is skipped;
/// none read at all is an error.
pub fn build(
    photos: &[PhotoRow],
    open: impl Fn(&Path) -> Result<RgbaImage, String>,
) -> Result<Sheet, String> {
    let (picked, all_edited) = pick_samples(photos, |r| r.edited, CAP);
    let mut sheet = Sheet {
        ids: Vec::with_capacity(picked.len()),
        bitmaps: Vec::with_capacity(picked.len()),
        all_edited,
        skipped: 0,
    };
    for row in picked {
        match open(&row.path) {
            Ok(bitmap) => {
                sheet.ids.push(row.id);
                sheet.bitmaps.push(bitmap);
            }
            Err(e) => {
                log::info!("match: photo {} could not be read: {e}", row.id);
                sheet.skipped += 1;
            }
        }
    }
    if sheet.bitmaps.is_empty() {
        return Err(NONE_READ.to_owned());
    }
    Ok(sheet)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::path::PathBuf;

    pub(crate) fn row(id: i64, edited: bool) -> PhotoRow {
        PhotoRow {
            id,
            folder_id: 1,
            path: PathBuf::from(format!("{id}.jpg")),
            width: None,
            height: None,
            camera: None,
            lens: None,
            captured_at: None,
            kind: "jpeg".into(),
            error: None,
            sidecar_error: None,
            missing: false,
            edited,
            updated_at: String::new(),
        }
    }

    #[test]
    fn build_picks_at_most_24_and_prefers_unedited() {
        // 30 photos; 3, 10 and 20 are edited; 7 cannot be read.
        let photos: Vec<PhotoRow> = (1..=30)
            .map(|id| row(id, [3, 10, 20].contains(&id)))
            .collect();
        let opened = RefCell::new(Vec::new());
        let sheet = build(&photos, |path| {
            opened.borrow_mut().push(path.to_path_buf());
            if path == Path::new("7.jpg") {
                return Err("broken".into());
            }
            Ok(RgbaImage::from_pixel(8, 6, image::Rgba([90, 90, 90, 255])))
        })
        .unwrap();
        assert_eq!(opened.borrow().len(), 24, "at most 24 opened");
        assert!(!sheet.all_edited);
        assert_eq!(sheet.skipped, 1);
        assert_eq!(sheet.ids.len(), 23);
        assert_eq!(sheet.bitmaps.len(), 23);
        assert!(
            sheet.ids.iter().all(|id| ![3, 10, 20, 7].contains(id)),
            "unedited only: {:?}",
            sheet.ids
        );
        assert_eq!(sheet.ids.first(), Some(&1));
        assert_eq!(sheet.ids.last(), Some(&30), "spread to the last one");
        let tiled = sheet.tiled();
        assert_eq!((tiled.width(), tiled.height()), (4 * 256, 6 * 256));

        // All edited: their files are read anyway, and the flag says so.
        let edited: Vec<PhotoRow> = (1..=5).map(|id| row(id, true)).collect();
        let sheet = build(&edited, |_| {
            Ok(RgbaImage::from_pixel(4, 4, image::Rgba([64, 64, 64, 255])))
        })
        .unwrap();
        assert!(sheet.all_edited);
        assert_eq!(sheet.ids, [1, 2, 3, 4, 5]);
        assert!((sheet.medians()[&2] - 64.0 / 255.0).abs() < 0.01);

        // Nothing readable.
        let err = build(&edited, |_| Err("no".into())).unwrap_err();
        assert_eq!(err, NONE_READ);
    }
}
