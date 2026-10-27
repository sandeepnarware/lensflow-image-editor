//! What culling looks like: the filter bar over the grid, and a cell's marks (stars under
//! the thumbnail, a flag and a colour dot in its top-right corner, rejects dimmed). The marks
//! are painted shapes rather than glyphs, so they never depend on a font having a star.

use super::cull::{Filter, FlagFilter, LabelFilter, Sort};
use crate::theme;
use eframe::egui::{self, Color32, Pos2, Rect, Shape, Stroke};
use lf_catalog::PhotoRow;
use lf_catalog::cull::{Flag, Label, MAX_RATING};

/// How much a rejected photo's thumbnail is dimmed (a tint multiplied into it).
pub const REJECTED_TINT: Color32 = Color32::from_gray(80);

/// A label's colour.
pub fn label_colour(label: Label) -> Color32 {
    match label {
        Label::Red => Color32::from_rgb(0xef, 0x44, 0x44),
        Label::Yellow => Color32::from_rgb(0xea, 0xb3, 0x08),
        Label::Green => Color32::from_rgb(0x22, 0xc5, 0x5e),
        Label::Blue => Color32::from_rgb(0x3b, 0x82, 0xf6),
        Label::Purple => Color32::from_rgb(0xa8, 0x55, 0xf7),
    }
}

/// The text a rating choice shows.
fn rating_name(min: u8) -> String {
    match min {
        0 => "Any rating".to_owned(),
        5 => "5 stars".to_owned(),
        1 => "1 star or more".to_owned(),
        n => format!("{n} stars or more"),
    }
}

/// The filter bar's lists, by the names screen readers (and the tests) know them.
pub const FLAG_LIST: &str = "Filter by flag";
pub const RATING_LIST: &str = "Filter by rating";
pub const LABEL_LIST: &str = "Filter by label";
pub const SORT_LIST: &str = "Sort order";

/// Names a list for screen readers: its shown text is the choice, not what it chooses.
fn named(response: &egui::Response, name: &str) {
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, name));
}

/// The bar over the grid; returns the filter when the user changed it.
pub fn filter_bar(ui: &mut egui::Ui, current: Filter) -> Option<Filter> {
    let mut next = current;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Show").color(theme::MUTED));
        let flag = egui::ComboBox::from_id_salt("filter-flag")
            .selected_text(next.flag.name())
            .show_ui(ui, |ui| {
                for f in FlagFilter::ALL {
                    ui.selectable_value(&mut next.flag, f, f.name());
                }
            });
        named(&flag.response, FLAG_LIST);
        let rating = egui::ComboBox::from_id_salt("filter-rating")
            .selected_text(rating_name(next.min_rating))
            .show_ui(ui, |ui| {
                for n in 0..=MAX_RATING {
                    ui.selectable_value(&mut next.min_rating, n, rating_name(n));
                }
            });
        named(&rating.response, RATING_LIST);
        let label = egui::ComboBox::from_id_salt("filter-label")
            .selected_text(next.label.name())
            .show_ui(ui, |ui| {
                for l in LabelFilter::all() {
                    ui.selectable_value(&mut next.label, l, l.name());
                }
            });
        named(&label.response, LABEL_LIST);
        if next.narrows() && ui.button("Clear filter").clicked() {
            next = Filter {
                sort: next.sort,
                ..Filter::default()
            };
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let sort = egui::ComboBox::from_id_salt("filter-sort")
                .selected_text(next.sort.name())
                .show_ui(ui, |ui| {
                    for s in Sort::ALL {
                        ui.selectable_value(&mut next.sort, s, s.name());
                    }
                });
            named(&sort.response, SORT_LIST);
            ui.label(egui::RichText::new("Sort by").color(theme::MUTED));
        });
    });
    ui.add_space(4.0);
    (next != current).then_some(next)
}

/// The marks a cell shows, and the words for its hover text and accessibility label.
pub fn describe(row: &PhotoRow) -> Option<String> {
    let mut parts = Vec::new();
    match row.flag {
        Flag::Picked => parts.push("Picked".to_owned()),
        Flag::Rejected => parts.push("Rejected".to_owned()),
        Flag::Unflagged => {}
    }
    match row.rating {
        0 => {}
        1 => parts.push("1 star".to_owned()),
        n => parts.push(format!("{n} stars")),
    }
    if let Some(label) = row.label {
        parts.push(format!("{} label", label.name()));
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// The rating's stars, ending at `right_bottom`. An unrated photo shows none, so the grid
/// stays quiet.
pub fn paint_stars(painter: &egui::Painter, right_bottom: Pos2, rating: u8, size: f32) {
    let step = size + 2.0;
    for i in 0..rating.min(MAX_RATING) {
        let centre = Pos2::new(
            right_bottom.x - size / 2.0 - f32::from(rating - 1 - i) * step,
            right_bottom.y - size / 2.0,
        );
        painter.add(star(centre, size / 2.0, theme::INK));
    }
}

/// A five-pointed star of outer radius `r`.
fn star(centre: Pos2, r: f32, fill: Color32) -> Shape {
    let inner = r * 0.45;
    // A star is not convex: draw it as five triangles around a pentagon.
    let point = |k: usize, radius: f32| {
        let a = -std::f32::consts::FRAC_PI_2 + k as f32 * std::f32::consts::PI / 5.0;
        centre + radius * egui::vec2(a.cos(), a.sin())
    };
    let mut shapes = vec![Shape::convex_polygon(
        (0..5).map(|k| point(2 * k + 1, inner)).collect(),
        fill,
        Stroke::NONE,
    )];
    for k in 0..5 {
        shapes.push(Shape::convex_polygon(
            vec![
                point(2 * k + 9, inner),
                point(2 * k, r),
                point(2 * k + 1, inner),
            ],
            fill,
            Stroke::NONE,
        ));
    }
    Shape::Vec(shapes)
}

/// The flag and label in a cell's top-right corner, right to left from `right_top`.
pub fn paint_corner(painter: &egui::Painter, right_top: Pos2, row: &PhotoRow, size: f32) {
    let mut x = right_top.x;
    if let Some(label) = row.label {
        let centre = Pos2::new(x - size / 2.0, right_top.y + size / 2.0);
        painter.circle(
            centre,
            size / 2.0,
            label_colour(label),
            Stroke::new(1.0, theme::BG),
        );
        x -= size + 4.0;
    }
    let area = Rect::from_min_size(Pos2::new(x - size, right_top.y), egui::Vec2::splat(size));
    match row.flag {
        Flag::Picked => {
            // A pennant on a pole.
            let pole = area.left() + size * 0.2;
            painter.line_segment(
                [Pos2::new(pole, area.top()), Pos2::new(pole, area.bottom())],
                Stroke::new(1.5, theme::SUCCESS),
            );
            painter.add(Shape::convex_polygon(
                vec![
                    Pos2::new(pole, area.top()),
                    Pos2::new(area.right(), area.top() + size * 0.3),
                    Pos2::new(pole, area.top() + size * 0.6),
                ],
                theme::SUCCESS,
                Stroke::NONE,
            ));
        }
        Flag::Rejected => {
            let s = Stroke::new(2.0, theme::DANGER);
            let a = area.shrink(size * 0.15);
            painter.line_segment([a.left_top(), a.right_bottom()], s);
            painter.line_segment([a.right_top(), a.left_bottom()], s);
        }
        Flag::Unflagged => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cell_says_its_marks() {
        let mut row = PhotoRow::default();
        assert_eq!(describe(&row), None);
        row.flag = Flag::Picked;
        row.rating = 3;
        row.label = Some(Label::Green);
        assert_eq!(
            describe(&row).as_deref(),
            Some("Picked, 3 stars, Green label")
        );
        row.flag = Flag::Rejected;
        row.rating = 1;
        row.label = None;
        assert_eq!(describe(&row).as_deref(), Some("Rejected, 1 star"));
    }

    #[test]
    fn rating_choices_read_well() {
        assert_eq!(rating_name(0), "Any rating");
        assert_eq!(rating_name(1), "1 star or more");
        assert_eq!(rating_name(3), "3 stars or more");
        assert_eq!(rating_name(5), "5 stars");
    }
}
