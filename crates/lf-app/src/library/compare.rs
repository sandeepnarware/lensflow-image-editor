//! Compare (`C`): two to four selected photos side by side, large, with their edits, zoomed
//! and panned together, to pick the sharpest of a burst. A click makes a photo the active
//! one; flags, stars and labels then go to it alone, and `←`/`→` move the active photo.
//! `Esc` or "Back to grid" returns.
//!
//! Each photo is decoded and developed on the `Open` lane (through the frame cache Develop
//! uses, so opening one afterwards is quick) and read back at up to [`EDGE`] pixels, then
//! shown as an egui texture. "1:1" is one of those pixels per screen pixel.

use super::{Env, Library, Mode};
use crate::jobs::{Lane, Msg};
use crate::theme;
use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Vec2};
use image::RgbaImage;
use lf_catalog::edits;
use lf_settings::{DevelopSettings, EditSettings};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// The long edge each photo is read back at.
pub const EDGE: u32 = 3000;
/// The most photos compared at once.
pub const MAX: usize = 4;
pub const BACK: &str = "Back to grid";
pub const HINT: &str = "Scroll to zoom, drag to pan (all together); click a photo to make it active; double-click for fit ⇄ 1:1.";

/// Rendered pixels on their way to the window.
pub struct Pixels(pub RgbaImage);

impl std::fmt::Debug for Pixels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pixels({}×{})", self.0.width(), self.0.height())
    }
}

/// Whether `n` selected photos can be compared.
pub fn can_compare(n: usize) -> bool {
    (2..=MAX).contains(&n)
}

/// Renders photo `id` with its saved edits at up to [`EDGE`] pixels.
pub fn render(
    gpu: &Arc<lf_pipeline::Gpu>,
    root: &Path,
    id: i64,
    path: &Path,
    cache: Option<&lf_raw::cache::FrameCache>,
    quality: lf_raw::demosaic::Quality,
) -> Result<RgbaImage, String> {
    let catalog = lf_catalog::Catalog::open(root).map_err(|e| e.to_string())?;
    let settings = edits::get_settings(&catalog, id)
        .map_err(|e| e.to_string())?
        .unwrap_or_else(EditSettings::default_settings);
    drop(catalog);
    let develop = DevelopSettings::from_settings(&settings);
    let max_edge = crate::develop::open::max_edge(gpu);
    let opened = match cache {
        Some(cache) => crate::develop::open::open_cached(path, quality, &develop, max_edge, cache),
        None => lf_raw::open::open(path, quality, &develop, max_edge),
    }
    .map_err(|e| e.to_string())?;
    let mut renderer = lf_raw::render::renderer_for(gpu, &opened).map_err(|e| e.to_string())?;
    let (w, h) = renderer.output_size(&settings);
    let edge = EDGE.min(w.max(h)).min(gpu.max_edge());
    renderer.pixels(&settings, edge).map_err(|e| e.to_string())
}

/// What a cell shows.
enum Cell {
    Waiting,
    /// Rendered, not yet a texture (messages are handled without the egui context).
    Pending(Box<Pixels>),
    Shown(egui::TextureHandle),
    Failed(String),
}

/// The open comparison.
pub struct Compare {
    pub ids: Vec<i64>,
    cells: HashMap<i64, Cell>,
    /// Answers for an earlier comparison are dropped.
    pub generation: u64,
    /// 1: fit; more is closer.
    zoom: f32,
    /// The point of each photo at the middle of its cell, in 0..1 of the photo.
    centre: Vec2,
}

impl Compare {
    fn new(ids: Vec<i64>, generation: u64) -> Compare {
        Compare {
            cells: ids.iter().map(|id| (*id, Cell::Waiting)).collect(),
            ids,
            generation,
            zoom: 1.0,
            centre: Vec2::splat(0.5),
        }
    }
}

/// Where a photo of `size` pixels sits in `cell` at `zoom` about `centre`: the screen rect it
/// fills and the part of the photo (uv) shown there. `centre` comes back clamped so the photo
/// never leaves its cell when zoomed in.
pub fn place(cell: Rect, size: Vec2, zoom: f32, centre: Vec2) -> (Rect, Rect, Vec2) {
    let fit = (cell.width() / size.x).min(cell.height() / size.y);
    let scale = fit * zoom.max(1.0);
    let shown = size * scale;
    // The part of the photo the cell has room for, in 0..1.
    let part = Vec2::new(
        (cell.width() / shown.x).min(1.0),
        (cell.height() / shown.y).min(1.0),
    );
    let centre = Vec2::new(
        centre.x.clamp(part.x / 2.0, 1.0 - part.x / 2.0),
        centre.y.clamp(part.y / 2.0, 1.0 - part.y / 2.0),
    );
    let uv = Rect::from_center_size(centre.to_pos2(), part);
    let screen = Rect::from_center_size(cell.center(), shown.min(cell.size()));
    (screen, uv, centre)
}

impl Library {
    pub fn comparing(&self) -> bool {
        self.compare.is_some() && self.mode == Mode::Library
    }

    /// `C`: compares the selected photos (two to four), in the grid's order.
    pub(super) fn open_compare(&mut self, env: &mut Env) {
        let ids = self.selected_in_order();
        if !can_compare(ids.len()) {
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        self.compare_generation += 1;
        let generation = self.compare_generation;
        let quality = crate::develop::open::quality(env.prefs.develop_quality);
        for &id in &ids {
            let Some(path) = self.photo(id).map(|p| p.path.clone()) else {
                continue;
            };
            let gpu = self.gpu.clone();
            let cache = self.frames.clone();
            let root = root.clone();
            env.jobs.spawn(Lane::Open, "compare", move || {
                let result = render(&gpu, &root, id, &path, cache.as_deref(), quality)
                    .map(|p| Box::new(Pixels(p)));
                Msg::CompareRendered {
                    generation,
                    id,
                    result,
                }
            });
        }
        if let Some(&first) = ids.first() {
            self.selection.set_active(first);
        }
        self.compare = Some(Compare::new(ids, generation));
    }

    pub(super) fn close_compare(&mut self) {
        self.compare = None;
        self.reveal_active = true;
    }

    pub(super) fn compare_rendered(
        &mut self,
        generation: u64,
        id: i64,
        result: Result<Box<Pixels>, String>,
    ) {
        let Some(compare) = &mut self.compare else {
            return;
        };
        if compare.generation != generation {
            return;
        }
        let cell = match result {
            Ok(pixels) => Cell::Pending(pixels),
            Err(e) => Cell::Failed(e),
        };
        compare.cells.insert(id, cell);
    }

    /// `←`/`→` in Compare: the active photo moves among the compared ones.
    pub(super) fn compare_step(&mut self, forward: bool) {
        let Some(compare) = &self.compare else {
            return;
        };
        let n = compare.ids.len();
        let at = self
            .selection
            .active()
            .and_then(|a| compare.ids.iter().position(|id| *id == a))
            .unwrap_or(0);
        let next = if forward {
            (at + 1) % n
        } else {
            (at + n - 1) % n
        };
        self.selection.set_active(compare.ids[next]);
    }

    /// The comparison, in the central panel.
    pub(super) fn compare_ui(&mut self, ui: &mut egui::Ui) {
        let Some(compare) = &mut self.compare else {
            return;
        };
        let mut back = false;
        for (id, cell) in compare.cells.iter_mut() {
            if let Cell::Pending(pixels) = cell {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [pixels.0.width() as usize, pixels.0.height() as usize],
                    pixels.0.as_raw(),
                );
                *cell = Cell::Shown(ui.ctx().load_texture(
                    format!("compare-{id}"),
                    image,
                    egui::TextureOptions::LINEAR,
                ));
            }
        }
        ui.horizontal(|ui| {
            if ui.button(format!("⏴ {BACK}")).clicked() {
                back = true;
            }
            ui.label(RichText::new(HINT).color(theme::MUTED).small());
        });
        ui.add_space(4.0);
        let n = compare.ids.len();
        let (cols, rows) = if n == 4 { (2, 2) } else { (n, 1) };
        let area = ui.available_rect_before_wrap();
        let caption = 22.0;
        let gap = 6.0;
        let cell_w = (area.width() - gap * (cols - 1) as f32) / cols as f32;
        let cell_h = (area.height() - gap * (rows - 1) as f32) / rows as f32;
        let active = self.selection.active();
        let mut clicked = None;
        let mut zoom_by = 1.0f32;
        let mut pan = Vec2::ZERO;
        let mut toggle = false;
        for (i, &id) in compare.ids.iter().enumerate() {
            let (c, r) = (i % cols, i / cols);
            let min = Pos2::new(
                area.left() + c as f32 * (cell_w + gap),
                area.top() + r as f32 * (cell_h + gap),
            );
            let outer = Rect::from_min_size(min, Vec2::new(cell_w, cell_h));
            let picture = Rect::from_min_max(outer.min, outer.max - Vec2::new(0.0, caption));
            let response = ui.interact(
                outer,
                ui.scope_id().with(("compare-cell", id)),
                Sense::click_and_drag(),
            );
            let painter = ui.painter_at(outer);
            painter.rect_filled(picture, 2.0, theme::BG);
            match compare.cells.get(&id) {
                Some(Cell::Shown(texture)) => {
                    let size = texture.size_vec2();
                    let (screen, uv, _) = place(picture, size, compare.zoom, compare.centre);
                    painter.image(texture.id(), screen, uv, Color32::WHITE);
                    if response.dragged() {
                        let fit = (picture.width() / size.x).min(picture.height() / size.y);
                        let scale = fit * compare.zoom;
                        pan -= response.drag_delta() / (size * scale);
                    }
                }
                Some(Cell::Failed(e)) => {
                    painter.text(
                        picture.center(),
                        egui::Align2::CENTER_CENTER,
                        e,
                        egui::FontId::proportional(13.0),
                        theme::WARNING,
                    );
                }
                _ => {
                    painter.text(
                        picture.center(),
                        egui::Align2::CENTER_CENTER,
                        "Rendering…",
                        egui::FontId::proportional(13.0),
                        theme::MUTED,
                    );
                }
            }
            if active == Some(id) {
                painter.rect_stroke(
                    picture.shrink(1.0),
                    2.0,
                    egui::Stroke::new(2.0, theme::INK),
                    egui::StrokeKind::Inside,
                );
            }
            if let Some(row) = self.rows.get(&id).map(|&i| &self.photos[i]) {
                let name = crate::develop::open::file_name(&row.path);
                let text = match super::cull_ui::describe(row) {
                    Some(marks) => format!("{name}  ·  {marks}"),
                    None => name,
                };
                painter.text(
                    Pos2::new(outer.left() + 4.0, outer.bottom() - caption / 2.0),
                    egui::Align2::LEFT_CENTER,
                    text,
                    egui::FontId::proportional(13.0),
                    theme::INK,
                );
            }
            if response.clicked() || response.drag_started() {
                clicked = Some(id);
            }
            if response.double_clicked() {
                toggle = true;
            }
            if response.hovered() {
                let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                if scroll != 0.0 {
                    zoom_by *= (scroll / 200.0).exp();
                }
            }
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Image, true, format!("Compare photo {id}"))
            });
        }
        ui.allocate_rect(area, Sense::hover());
        if toggle {
            compare.zoom = if compare.zoom > 1.01 {
                1.0
            } else {
                one_to_one(compare, area, cols, rows)
            };
        }
        compare.zoom = (compare.zoom * zoom_by).clamp(1.0, 16.0);
        compare.centre += pan;
        compare.centre = compare.centre.clamp(Vec2::ZERO, Vec2::splat(1.0));
        if let Some(id) = clicked {
            self.selection.set_active(id);
        }
        if back {
            self.close_compare();
        }
    }
}

/// The zoom that shows the first rendered photo at one pixel per screen pixel.
fn one_to_one(compare: &Compare, area: Rect, cols: usize, rows: usize) -> f32 {
    let cell = Vec2::new(
        area.width() / cols as f32,
        area.height() / rows as f32 - 22.0,
    );
    compare
        .ids
        .iter()
        .find_map(|id| match compare.cells.get(id) {
            Some(Cell::Shown(t)) => {
                let size = t.size_vec2();
                Some(1.0 / (cell.x / size.x).min(cell.y / size.y))
            }
            _ => None,
        })
        .unwrap_or(4.0)
        .max(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_to_four_photos_can_be_compared() {
        assert!(!can_compare(1));
        assert!(can_compare(2) && can_compare(4));
        assert!(!can_compare(5));
    }

    #[test]
    fn a_fitted_photo_fills_its_cell_and_a_zoomed_one_shows_a_part() {
        let cell = Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0));
        let size = Vec2::new(4000.0, 3000.0);
        let (screen, uv, _) = place(cell, size, 1.0, Vec2::splat(0.5));
        assert_eq!(screen, cell);
        assert_eq!(uv, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)));
        // 10× in, centred on the top-left corner: clamped to stay inside the photo.
        let (screen, uv, centre) = place(cell, size, 10.0, Vec2::ZERO);
        assert_eq!(screen, cell);
        assert!((uv.width() - 0.1).abs() < 1e-6);
        assert!((centre.x - 0.05).abs() < 1e-6 && uv.min.x.abs() < 1e-6);
    }

    #[test]
    fn a_photo_is_rendered_with_its_edits() {
        let Some(gpu) = lf_pipeline::test_support::try_shared_gpu() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.png");
        image::RgbaImage::from_pixel(64, 48, image::Rgba([100, 100, 100, 255]))
            .save(&path)
            .unwrap();
        let catalog = lf_catalog::Catalog::open(dir.path()).unwrap();
        let folder = catalog.add_folder(&dir.path().to_string_lossy()).unwrap();
        lf_catalog::import::run(
            &catalog,
            folder,
            &lf_catalog::import::scan(dir.path()).candidates,
            200,
            &mut |_| Ok(()),
        )
        .unwrap();
        let id = catalog.photo_ids().unwrap()[0];
        let mut brighter = EditSettings::default_settings();
        brighter.exposure = 1.0;
        edits::set_settings(&catalog, id, &brighter).unwrap();
        let plain = image::open(&path).unwrap().to_rgba8();
        let shown = render(
            &gpu,
            dir.path(),
            id,
            &path,
            None,
            lf_raw::demosaic::Quality::Fast,
        )
        .unwrap();
        assert_eq!(shown.dimensions(), (64, 48));
        assert!(shown.get_pixel(10, 10).0[0] > plain.get_pixel(10, 10).0[0]);
    }

    #[test]
    fn a_tall_photo_is_centred_in_a_wide_cell() {
        let cell = Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0));
        let (screen, _, _) = place(cell, Vec2::new(2000.0, 3000.0), 1.0, Vec2::splat(0.5));
        assert_eq!(screen.size(), Vec2::new(200.0, 300.0));
        assert_eq!(screen.center(), cell.center());
    }
}
