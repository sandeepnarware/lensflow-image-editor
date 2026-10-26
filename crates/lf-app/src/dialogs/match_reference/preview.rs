//! The before/after of one sampled photo, as the web's preview: a square of the unedited
//! contact sheet, and the same square of the sheet rendered with the grade at the chosen
//! strength. The render waits 120 ms after the last change of strength or photo.

use super::steps::SheetRender;
use eframe::egui::{self, TextureHandle, TextureOptions};
use image::RgbaImage;
use image::imageops;
use lf_grade::apply_strength;
use lf_grade::sheet::{SHEET_COLUMNS, TILE_EDGE, tile_layout};
use lf_settings::EditSettings;
use std::time::{Duration, Instant};

/// How long the preview waits after the last change.
pub const SETTLE: Duration = Duration::from_millis(120);

/// The square of tile `index` (of `tiles`) cut from `img`, a render of the `sheet_w` wide
/// sheet at any size.
pub fn slice(img: &RgbaImage, tiles: usize, index: usize, sheet_w: u32) -> RgbaImage {
    let layout = tile_layout(tiles, TILE_EDGE, SHEET_COLUMNS);
    let (x, y) = layout
        .rects
        .get(index)
        .or(layout.rects.first())
        .copied()
        .unwrap_or((0, 0));
    let k = f64::from(img.width()) / f64::from(sheet_w.max(1));
    let at = |v: u32| (f64::from(v) * k).round() as u32;
    let edge = at(TILE_EDGE).max(1);
    let (x, y) = (at(x).min(img.width() - 1), at(y).min(img.height() - 1));
    let w = edge.min(img.width() - x);
    let h = edge.min(img.height() - y);
    imageops::crop_imm(img, x, y, w, h).to_image()
}

/// The preview's textures and what they show.
#[derive(Default)]
pub struct Preview {
    pub index: usize,
    before: Option<TextureHandle>,
    after: Option<TextureHandle>,
    /// The (photo, strength) the textures show, and the one wanted since when.
    shown: Option<(usize, u32)>,
    wanted: Option<((usize, u32), Instant)>,
    pub error: Option<String>,
}

impl Preview {
    /// Renders again when the photo or strength changed and has settled.
    pub fn update(
        &mut self,
        ctx: &egui::Context,
        renderer: &mut dyn SheetRender,
        sheet: &RgbaImage,
        tiles: usize,
        grade: &EditSettings,
        strength: f64,
    ) {
        let key = (self.index, strength.round() as u32);
        if self.shown == Some(key) {
            self.wanted = None;
            return;
        }
        let now = Instant::now();
        let since = match self.wanted {
            Some((k, at)) if k == key => at,
            _ => {
                self.wanted = Some((key, now));
                now
            }
        };
        // The first picture shows at once; later changes settle first.
        if self.shown.is_some() && now.duration_since(since) < SETTLE {
            ctx.request_repaint_after(SETTLE);
            return;
        }
        let texture = |name: &str, img: &RgbaImage| {
            let size = [img.width() as usize, img.height() as usize];
            let pixels = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
            ctx.load_texture(name, pixels, TextureOptions::LINEAR)
        };
        let sheet_w = sheet.width();
        if self.shown.map(|(i, _)| i) != Some(self.index) || self.before.is_none() {
            self.before = Some(texture(
                "match-before",
                &slice(sheet, tiles, self.index, sheet_w),
            ));
        }
        let long = sheet.width().max(sheet.height());
        match renderer.pixels(&apply_strength(grade, strength), long) {
            Ok(img) => {
                self.after = Some(texture(
                    "match-after",
                    &slice(&img, tiles, self.index, sheet_w),
                ));
                self.error = None;
            }
            Err(e) => self.error = Some(format!("The preview could not be drawn. {e}")),
        }
        self.shown = Some(key);
        self.wanted = None;
    }

    /// The texture to show: before while held.
    pub fn texture(&self, before: bool) -> Option<&TextureHandle> {
        if before {
            self.before.as_ref().or(self.after.as_ref())
        } else {
            self.after.as_ref()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slice_is_its_tiles_square_at_any_render_size() {
        // Five tiles: two rows of four columns (1024 x 512); tile 5 is at (256, 256).
        let sheet = RgbaImage::from_fn(1024, 512, |x, y| {
            image::Rgba([(x / 256) as u8, (y / 256) as u8, 0, 255])
        });
        let s = slice(&sheet, 6, 5, 1024);
        assert_eq!((s.width(), s.height()), (256, 256));
        assert_eq!(s.get_pixel(10, 10).0[..2], [1, 1]);
        // A render at half size: the same square, halved.
        let half = imageops::resize(&sheet, 512, 256, imageops::FilterType::Nearest);
        let s = slice(&half, 6, 5, 1024);
        assert_eq!((s.width(), s.height()), (128, 128));
        assert_eq!(s.get_pixel(10, 10).0[..2], [1, 1]);
        // An index past the end falls back to the first tile.
        assert_eq!(slice(&sheet, 6, 99, 1024).get_pixel(0, 0).0[..2], [0, 0]);
    }
}
