//! The filmstrip: one row of small thumbnails for the current folder, shared by Library and
//! Develop. The active photo is outlined in the brand colour; a click selects; the wheel
//! scrolls it sideways.

use super::Library;
use super::grid::{Look, paint_cell};
use eframe::egui::{self, Rect, Sense};

/// A filmstrip thumbnail's edge, in points.
pub const THUMB: f32 = 96.0;
/// The space between thumbnails.
pub const GAP: f32 = 4.0;
/// The filmstrip panel's height (thumbnails, margins and the scroll bar).
pub const HEIGHT: f32 = THUMB + 24.0;

/// The thumbnails `first..last` (end exclusive) of `count` visible in a strip scrolled
/// right by `scroll_x` and `viewport_w` wide.
pub fn visible(scroll_x: f32, viewport_w: f32, count: usize) -> (usize, usize) {
    let step = THUMB + GAP;
    let first = ((scroll_x.max(0.0) / step).floor() as usize).min(count);
    let last = (((scroll_x.max(0.0) + viewport_w.max(0.0)) / step).ceil() as usize).min(count);
    (first, last)
}

/// Draws the strip; returns the photo clicked, if any.
pub fn show(ui: &mut egui::Ui, lib: &mut Library) -> Option<i64> {
    if lib.order.is_empty() {
        return None;
    }
    // The vertical wheel scrolls the strip sideways.
    ui.style_mut().always_scroll_the_only_direction = true;
    let reveal = if lib.reveal_active {
        lib.selection
            .active()
            .and_then(|id| lib.index.get(&id).copied())
    } else {
        None
    };
    let active = lib.selection.active();
    let mut clicked = None;
    egui::ScrollArea::horizontal()
        .id_salt("filmstrip")
        .auto_shrink(false)
        .show_viewport(ui, |ui, viewport| {
            let origin = ui.max_rect().min;
            let n = lib.order.len();
            let step = THUMB + GAP;
            ui.set_min_size(egui::vec2(n as f32 * step - GAP, THUMB));
            let rect_of = |i: usize| {
                Rect::from_min_size(
                    origin + egui::vec2(i as f32 * step, 0.0),
                    egui::Vec2::splat(THUMB),
                )
            };
            if let Some(i) = reveal {
                ui.scroll_to_rect(rect_of(i), Some(egui::Align::Center));
            }
            let (first, last) = visible(viewport.min.x, viewport.width(), n);
            let ids = lib.order[first..last].to_vec();
            lib.thumbs.strip(&ids);
            for (offset, id) in ids.into_iter().enumerate() {
                let rect = rect_of(first + offset);
                let response = ui.interact(rect, ui.scope_id().with(("strip", id)), Sense::click());
                let look = Look {
                    selected: false,
                    active: active == Some(id),
                    hovered: response.hovered(),
                };
                if let Some(text) = paint_cell(ui, lib, id, rect, look, false) {
                    response.clone().on_hover_text(text);
                }
                if response.clicked() {
                    clicked = Some(id);
                }
            }
        });
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_shows_only_what_fits() {
        assert_eq!(visible(0.0, 1000.0, 5_000), (0, 10));
        assert_eq!(visible(1000.0, 1000.0, 5_000), (10, 20));
        assert_eq!(visible(0.0, 1000.0, 3), (0, 3));
    }
}
