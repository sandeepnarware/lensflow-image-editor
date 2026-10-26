//! The library grid's geometry: which cells of a long list are on screen (only those are
//! drawn), as a pure function of the viewport, and the drawing of those cells.

use super::{FAILED_BADGE, Library, MISSING_BADGE, SIDECAR_BADGE};
use crate::theme;
use eframe::egui::{self, Color32, Rect, Sense, Stroke};
use std::collections::HashSet;

/// A grid cell's edge, in points.
pub const CELL: f32 = 200.0;
/// The space between cells, in points.
pub const GAP: f32 = 8.0;

/// The cells on screen: indices `first..last` (end exclusive) of the photo list.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Visible {
    pub first: usize,
    pub last: usize,
    pub columns: usize,
    /// A row's height including the gap below it.
    pub row_height: f32,
    /// The height of every row together (the scroll area's content height).
    pub total_height: f32,
}

pub struct Layout;

impl Layout {
    /// The visible cells of `count` cells of `cell` points with `gap` between them, laid out
    /// in as many columns as fit `viewport_w`, scrolled down by `scroll_y`.
    pub fn compute(
        viewport_w: f32,
        scroll_y: f32,
        viewport_h: f32,
        count: usize,
        cell: f32,
        gap: f32,
    ) -> Visible {
        let row_height = cell + gap;
        let columns = (((viewport_w + gap) / row_height).floor() as usize).max(1);
        let rows = count.div_ceil(columns);
        let total_height = if rows == 0 {
            0.0
        } else {
            rows as f32 * row_height - gap
        };
        let top = scroll_y.max(0.0);
        let first_row = ((top / row_height).floor() as usize).min(rows);
        let last_row = (((top + viewport_h.max(0.0)) / row_height).ceil() as usize).min(rows);
        Visible {
            first: (first_row * columns).min(count),
            last: (last_row * columns).min(count),
            columns,
            row_height,
            total_height,
        }
    }

    /// The top-left corner of cell `index`, relative to the grid's top-left.
    pub fn cell_origin(index: usize, columns: usize, cell: f32, gap: f32) -> (f32, f32) {
        let columns = columns.max(1);
        let step = cell + gap;
        (
            (index % columns) as f32 * step,
            (index / columns) as f32 * step,
        )
    }
}

/// What happened in the grid this frame.
pub struct GridActions {
    /// How far down the grid is scrolled.
    pub scroll_y: f32,
    /// A cell was double-clicked: open this photo.
    pub open: Option<i64>,
}

/// Draws the visible cells of the current folder and applies clicks to the selection.
pub fn show(ui: &mut egui::Ui, lib: &mut Library) -> GridActions {
    let mut open = None;
    let reveal = if lib.reveal_active {
        lib.selection
            .active()
            .and_then(|id| lib.index.get(&id).copied())
    } else {
        None
    };
    let selected: HashSet<i64> = lib.selection.current.iter().copied().collect();
    let active = lib.selection.active();
    let scroll_y = egui::ScrollArea::vertical()
        .id_salt("library-grid")
        .auto_shrink(false)
        .show_viewport(ui, |ui, viewport| {
            let origin = ui.max_rect().min;
            let width = ui.available_width();
            let v = Layout::compute(
                width,
                viewport.min.y,
                viewport.height(),
                lib.order.len(),
                CELL,
                GAP,
            );
            lib.columns = v.columns;
            ui.set_min_size(egui::vec2(width, v.total_height));
            let cell_rect = |i: usize| {
                let (x, y) = Layout::cell_origin(i, v.columns, CELL, GAP);
                Rect::from_min_size(origin + egui::vec2(x, y), egui::Vec2::splat(CELL))
            };
            if let Some(i) = reveal {
                ui.scroll_to_rect(cell_rect(i), None);
            }
            lib.thumbs.visible(&lib.order, v.first, v.last);
            let modifiers = ui.input(|i| i.modifiers);
            for i in v.first..v.last {
                let id = lib.order[i];
                let rect = cell_rect(i);
                let response = ui.interact(rect, ui.scope_id().with(("cell", id)), Sense::click());
                let look = Look {
                    selected: selected.contains(&id),
                    active: active == Some(id),
                    hovered: response.hovered(),
                };
                let hover = paint_cell(ui, lib, id, rect, look, true);
                let response = match hover {
                    Some(text) => response.on_hover_text(text),
                    None => response,
                };
                if response.double_clicked() {
                    open = Some(id);
                } else if response.clicked() {
                    if modifiers.command {
                        lib.selection.ctrl_click(id);
                    } else if modifiers.shift {
                        lib.selection.shift_click(&lib.order, id);
                    } else {
                        lib.selection.click(id);
                    }
                }
            }
            viewport.min.y
        })
        .inner;
    GridActions { scroll_y, open }
}

/// How a cell is outlined.
#[derive(Clone, Copy)]
pub(super) struct Look {
    pub selected: bool,
    pub active: bool,
    pub hovered: bool,
}

/// Paints one photo's cell: its thumbnail fitted inside, the outline, the badges and (in
/// the grid) the file name. Returns the text to show on hover (why it has a badge).
pub(super) fn paint_cell(
    ui: &egui::Ui,
    lib: &mut Library,
    id: i64,
    rect: Rect,
    look: Look,
    with_name: bool,
) -> Option<String> {
    let painter = ui.painter_at(rect.expand(2.0));
    let fill = if look.selected {
        theme::LINE
    } else {
        theme::SURFACE_2
    };
    painter.rect_filled(rect, 6.0, fill);
    let name_h = if with_name { 18.0 } else { 0.0 };
    let pad = if with_name { 8.0 } else { 3.0 };
    let area = Rect::from_min_max(
        rect.min + egui::vec2(pad, pad),
        rect.max - egui::vec2(pad, pad + name_h),
    );
    if let Some((texture, size)) = lib.thumbs.texture(id) {
        let scale = (area.width() / size.x).min(area.height() / size.y);
        let shown = Rect::from_center_size(area.center(), size * scale);
        painter.image(
            texture,
            shown,
            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }
    let photo = lib.photo(id);
    if with_name && let Some(photo) = photo {
        let name = photo
            .path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        painter.text(
            egui::pos2(rect.left() + pad, rect.bottom() - pad),
            egui::Align2::LEFT_BOTTOM,
            name,
            egui::FontId::proportional(12.0),
            theme::MUTED,
        );
    }
    let stroke = if look.active {
        Some(Stroke::new(3.0, theme::BRAND))
    } else if look.selected {
        Some(Stroke::new(1.5, theme::BRAND))
    } else if look.hovered {
        Some(Stroke::new(1.0, theme::MUTED))
    } else {
        None
    };
    if let Some(stroke) = stroke {
        painter.rect_stroke(rect, 6.0, stroke, egui::StrokeKind::Inside);
    }

    let mut badges: Vec<(&str, Color32)> = Vec::new();
    let mut hover = Vec::new();
    let missing = photo.is_some_and(|p| p.missing);
    let sidecar = photo.and_then(|p| p.sidecar_error.clone());
    if missing {
        badges.push((MISSING_BADGE, theme::DANGER));
        hover.push("The file is not in its folder any more.".to_owned());
    }
    if let Some(error) = lib.thumbs.error(id) {
        badges.push((FAILED_BADGE, theme::DANGER));
        hover.push(error.to_owned());
    }
    if let Some(error) = sidecar {
        badges.push((SIDECAR_BADGE, theme::WARNING));
        hover.push(error);
    }
    let mut at = rect.min + egui::vec2(pad + 2.0, pad + 2.0);
    let font = egui::FontId::proportional(if with_name { 12.0 } else { 10.0 });
    for (text, colour) in badges {
        let galley = painter.layout_no_wrap(text.to_owned(), font.clone(), theme::BG);
        let badge = Rect::from_min_size(at, galley.size() + egui::vec2(8.0, 4.0));
        painter.rect_filled(badge, 4.0, colour);
        painter.galley(badge.min + egui::vec2(4.0, 2.0), galley, theme::BG);
        at.y = badge.bottom() + 3.0;
    }
    (!hover.is_empty()).then(|| hover.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_virtualises_rows() {
        let (w, h) = (1000.0, 800.0);
        let rows_visible = (h / (CELL + GAP)).ceil() as usize;
        for scroll in [0.0, 100.0, 5_000.0, 123_456.7] {
            let v = Layout::compute(w, scroll, h, 10_000, CELL, GAP);
            assert_eq!(v.columns, 4, "four 200 px cells and their gaps fit 1000 px");
            assert!(v.first <= v.last && v.last <= 10_000);
            assert!(
                v.last - v.first <= v.columns * (rows_visible + 1),
                "{scroll}: {v:?}"
            );
            assert_eq!(v.first % v.columns, 0, "whole rows");
            // The first visible row is the one at the top of the viewport.
            let top_row = (scroll / v.row_height).floor() as usize;
            assert_eq!(v.first, (top_row * v.columns).min(10_000), "{scroll}");
        }
        let v = Layout::compute(w, 0.0, h, 10_000, CELL, GAP);
        assert_eq!(v.first, 0);
        assert_eq!(v.row_height, CELL + GAP);
        assert_eq!(v.total_height, 2_500.0 * (CELL + GAP) - GAP);
    }

    #[test]
    fn layout_at_the_end_and_with_few_or_no_cells() {
        let end = Layout::compute(1000.0, 2_500.0 * 208.0 - 800.0, 800.0, 10_000, CELL, GAP);
        assert_eq!(end.last, 10_000);
        let few = Layout::compute(1000.0, 0.0, 800.0, 3, CELL, GAP);
        assert_eq!((few.first, few.last), (0, 3));
        assert_eq!(few.total_height, CELL);
        let none = Layout::compute(1000.0, 0.0, 800.0, 0, CELL, GAP);
        assert_eq!((none.first, none.last, none.total_height), (0, 0, 0.0));
        // Narrower than one cell still shows one column.
        assert_eq!(Layout::compute(50.0, 0.0, 800.0, 10, CELL, GAP).columns, 1);
    }
}
