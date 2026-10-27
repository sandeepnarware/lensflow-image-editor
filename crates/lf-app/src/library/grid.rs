//! The library grid's geometry: which cells of a long list are on screen (only those are
//! drawn), as a pure function of the viewport, the drawing of those cells, and the box a drag
//! draws to select several at once.

use super::{FAILED_BADGE, Library, MISSING_BADGE, SIDECAR_BADGE};
use crate::theme;
use eframe::egui::{self, Color32, Rect, Sense, Stroke};
use std::collections::HashSet;

/// A grid cell's edge, in points.
pub const CELL: f32 = 200.0;
/// The space between cells, in points.
pub const GAP: f32 = 8.0;
/// The smallest box, in points, that counts as a marquee rather than a nudge of the mouse.
pub const MARQUEE_MIN: f32 = 4.0;

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

/// A selection box being dragged over the grid. Ctrl+drag keeps what was already selected.
#[derive(Debug, Clone)]
pub struct Marquee {
    /// Where the drag started, in screen points.
    pub start: egui::Pos2,
    /// Where the pointer is now.
    pub current: egui::Pos2,
    /// Ctrl was held: the box adds to the selection instead of replacing it.
    pub additive: bool,
    /// The selection when the box started (`additive` only).
    pub base: Vec<i64>,
    /// The box has grown past [`MARQUEE_MIN`], so it owns the drag.
    pub live: bool,
}

impl Marquee {
    /// The box between the two corners, whichever way the drag went.
    pub fn rect(&self) -> Rect {
        Rect::from_two_pos(self.start, self.current)
    }

    /// Grown enough to be a box rather than a shaky click.
    fn big_enough(&self) -> bool {
        let rect = self.rect();
        rect.width() >= MARQUEE_MIN || rect.height() >= MARQUEE_MIN
    }
}

/// The selection a `live` box leaves in `selection`: the photos its `ids` name, or those added
/// to the ones selected when it started.
fn apply_marquee(selection: &mut super::selection::Selection, marquee: &Marquee, ids: &[i64]) {
    if marquee.additive {
        selection.current = marquee.base.clone();
        selection.add_all(ids);
    } else {
        selection.select(ids);
    }
}

/// Draws the visible cells of the current folder and applies clicks and drags to the
/// selection.
pub fn show(ui: &mut egui::Ui, lib: &mut Library) -> GridActions {
    let mut open = None;
    let reveal = if lib.reveal_active {
        lib.selection
            .active()
            .and_then(|id| lib.index.get(&id).copied())
    } else {
        None
    };
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
            let cell_rect = |i: usize| cell_box(i, v.columns, origin);
            if let Some(i) = reveal {
                ui.scroll_to_rect(cell_rect(i), None);
            }
            // A box started last frame follows the pointer here, before the cells are drawn,
            // so the highlight is already in step with the pointer this frame.
            follow_marquee(ui, lib, v.columns, origin, v.first, v.last);
            let selected: HashSet<i64> = lib.selection.current.iter().copied().collect();
            let active = lib.selection.active();
            lib.thumbs.visible(&lib.order, v.first, v.last);
            let modifiers = ui.input(|i| i.modifiers);
            // A drag that starts in the empty space (below the last row, beside the last
            // column) draws a box too. The cells are made after it, so they stay on top.
            let backdrop = ui.interact(
                ui.max_rect(),
                ui.scope_id().with("grid-backdrop"),
                Sense::drag(),
            );
            if backdrop.drag_started_by(egui::PointerButton::Primary) {
                begin_marquee(ui, lib, modifiers.command);
            }
            for i in v.first..v.last {
                let id = lib.order[i];
                let rect = cell_rect(i);
                let response = ui.interact(
                    rect,
                    ui.scope_id().with(("cell", id)),
                    Sense::click_and_drag(),
                );
                let name = lib
                    .photo(id)
                    .map(|photo| crate::develop::open::file_name(&photo.path))
                    .unwrap_or_default();
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Button, true, name.clone())
                });
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
                if response.drag_started_by(egui::PointerButton::Primary) {
                    begin_marquee(ui, lib, modifiers.command);
                } else if response.double_clicked() {
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
            paint_marquee(ui, lib);
            viewport.min.y
        })
        .inner;
    GridActions { scroll_y, open }
}

/// A cell's rectangle on screen, given the grid's top-left corner.
pub fn cell_box(index: usize, columns: usize, origin: egui::Pos2) -> Rect {
    let (x, y) = Layout::cell_origin(index, columns, CELL, GAP);
    Rect::from_min_size(origin + egui::vec2(x, y), egui::Vec2::splat(CELL))
}

/// The photo ids whose cells `box` touches, in grid order. Only the cells drawn this frame
/// (`first..last`) can be found: a box dragged past the bottom of the screen selects what is
/// on it.
pub fn cells_in(
    area: Rect,
    columns: usize,
    first: usize,
    last: usize,
    origin: egui::Pos2,
    count: usize,
) -> Vec<usize> {
    (first..last.min(count))
        .filter(|&i| cell_box(i, columns, origin).intersects(area))
        .collect()
}

/// Starts a box where the button went down: Ctrl keeps what was already selected.
fn begin_marquee(ui: &egui::Ui, lib: &mut Library, additive: bool) {
    let Some(start) = ui
        .input(|i| i.pointer.press_origin())
        .or_else(|| ui.input(|i| i.pointer.latest_pos()))
    else {
        return;
    };
    let base = lib.selection.current.clone();
    lib.marquee = Some(Marquee {
        start,
        current: start,
        additive,
        base,
        live: false,
    });
}

/// Moves the box with the pointer, applies it to the selection, and drops it when the button
/// comes up.
fn follow_marquee(
    ui: &egui::Ui,
    lib: &mut Library,
    columns: usize,
    origin: egui::Pos2,
    first: usize,
    last: usize,
) {
    let Some(mut marquee) = lib.marquee.take() else {
        return;
    };
    let (pointer, down) = ui.input(|i| (i.pointer.latest_pos(), i.pointer.primary_down()));
    if let Some(pointer) = pointer {
        marquee.current = pointer;
    }
    marquee.live |= marquee.big_enough();
    if marquee.live {
        let indexes = cells_in(
            marquee.rect(),
            columns,
            first,
            last,
            origin,
            lib.order.len(),
        );
        let ids: Vec<i64> = indexes.into_iter().map(|i| lib.order[i]).collect();
        apply_marquee(&mut lib.selection, &marquee, &ids);
    }
    if down {
        lib.marquee = Some(marquee);
    }
}

/// The box itself: a translucent brand wash with a firm edge, over the cells it covers.
fn paint_marquee(ui: &egui::Ui, lib: &Library) {
    let Some(marquee) = &lib.marquee else {
        return;
    };
    if !marquee.live {
        return;
    }
    let rect = marquee.rect().intersect(ui.max_rect());
    if !rect.is_positive() {
        return;
    }
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, theme::BRAND.gamma_multiply(0.14));
    painter.rect_stroke(
        rect,
        2.0,
        Stroke::new(1.0, theme::BRAND),
        egui::StrokeKind::Inside,
    );
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

    #[test]
    fn a_box_finds_the_cells_it_touches_row_by_row() {
        // Four columns of 200 px with 8 px gaps, from the origin.
        let (columns, origin, count) = (4, egui::pos2(0.0, 0.0), 12);
        let step = CELL + GAP;
        // Over the first two cells of the first row only.
        let top = Rect::from_min_max(
            egui::pos2(10.0, 10.0),
            egui::pos2(step + CELL - 10.0, CELL - 10.0),
        );
        assert_eq!(cells_in(top, columns, 0, count, origin, count), [0, 1]);
        // A tall, narrow box over one column crosses rows: 1 and 5, not a range of 1..=5.
        let column = Rect::from_min_max(
            egui::pos2(step + 10.0, 10.0),
            egui::pos2(step + CELL - 10.0, 2.0 * step - 10.0),
        );
        assert_eq!(cells_in(column, columns, 0, count, origin, count), [1, 5]);
        // The whole grid, but only the cells drawn this frame are found.
        let all = Rect::from_min_max(origin, egui::pos2(4.0 * step, 3.0 * step));
        assert_eq!(cells_in(all, columns, 0, count, origin, count).len(), 12);
        assert_eq!(cells_in(all, columns, 4, 8, origin, count), [4, 5, 6, 7]);
        // A box in a gap touches nothing.
        let gap = Rect::from_min_max(
            egui::pos2(CELL + 1.0, CELL + 1.0),
            egui::pos2(CELL + GAP - 1.0, CELL + GAP - 1.0),
        );
        assert!(cells_in(gap, columns, 0, count, origin, count).is_empty());
        // Screen origin moves with the scroll area, not with the scroll position.
        let moved = egui::pos2(500.0, 40.0);
        let boxed = Rect::from_min_max(moved, moved + egui::vec2(CELL, CELL));
        assert_eq!(cells_in(boxed, columns, 0, count, moved, count), [0]);
    }

    #[test]
    fn a_nudge_is_not_a_box() {
        let at = egui::pos2(20.0, 20.0);
        let mut marquee = Marquee {
            start: at,
            current: at + egui::vec2(MARQUEE_MIN - 1.0, 0.0),
            additive: false,
            base: Vec::new(),
            live: false,
        };
        assert!(!marquee.big_enough());
        marquee.current = at + egui::vec2(0.0, MARQUEE_MIN);
        assert!(marquee.big_enough());
        // Dragged up and left: the rectangle still spans both corners.
        marquee.current = at - egui::vec2(50.0, 30.0);
        assert_eq!(marquee.rect().width(), 50.0);
        assert_eq!(marquee.rect().height(), 30.0);
    }

    #[test]
    fn ctrl_drag_adds_to_what_was_selected() {
        let mut selection = super::super::selection::Selection::default();
        selection.click(11);
        let base = selection.current.clone();
        let mut marquee = Marquee {
            start: egui::pos2(0.0, 0.0),
            current: egui::pos2(10.0, 10.0),
            additive: true,
            base,
            live: true,
        };
        apply_marquee(&mut selection, &marquee, &[20, 21]);
        assert_eq!(selection.current, [11, 20, 21]);
        // The next frame replaces what the box had added rather than piling it up.
        apply_marquee(&mut selection, &marquee, &[20]);
        assert_eq!(selection.current, [11, 20]);
        apply_marquee(&mut selection, &marquee, &[]);
        assert_eq!(selection.current, [11], "the base is what was selected");
        // Without Ctrl the box is the whole selection.
        marquee.additive = false;
        apply_marquee(&mut selection, &marquee, &[5, 6]);
        assert_eq!(selection.current, [5, 6]);
        assert_eq!(selection.active(), Some(6));
        apply_marquee(&mut selection, &marquee, &[]);
        assert!(selection.current.is_empty(), "an empty box clears");
    }
}
