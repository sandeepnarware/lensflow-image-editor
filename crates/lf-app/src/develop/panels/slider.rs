//! One adjustment slider, after the web's `EditorSlider.vue`: the label on the left, the value
//! on the right (click it to type), a track below that fills from the neutral value. Drag
//! anywhere on the row (`Shift` drags ten times finer), double-click resets to neutral, and
//! the arrow keys step it while it has keyboard focus. `changed` comes on every frame the
//! value moves (the preview follows); `released` once at the end of the gesture (commit).

use crate::theme;
use eframe::egui::{
    self, Color32, CornerRadius, FontId, Key, Modifiers, Rect, Sense, Stroke, TextEdit, WidgetInfo,
    pos2, vec2,
};

/// A slider's range, step and how its value reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Range {
    pub min: f64,
    pub max: f64,
    pub step: f64,
    /// Decimals shown (and kept: values snap to the step, then round to these).
    pub decimals: usize,
    /// Where double-click puts it, and where the track's fill starts.
    pub neutral: f64,
    /// Shown after the value (`°` for hues).
    pub unit: &'static str,
}

impl Range {
    /// Whole steps from `min` to `max`, neutral 0.
    pub const fn new(min: f64, max: f64) -> Range {
        Range {
            min,
            max,
            step: 1.0,
            decimals: 0,
            neutral: 0.0,
            unit: "",
        }
    }

    /// −100..100, neutral 0 (most sliders).
    pub const BIPOLAR: Range = Range::new(-100.0, 100.0);
    /// 0..100, neutral 0.
    pub const AMOUNT: Range = Range::new(0.0, 100.0);

    pub const fn step(mut self, step: f64, decimals: usize) -> Range {
        self.step = step;
        self.decimals = decimals;
        self
    }

    pub const fn neutral(mut self, neutral: f64) -> Range {
        self.neutral = neutral;
        self
    }

    pub const fn unit(mut self, unit: &'static str) -> Range {
        self.unit = unit;
        self
    }

    /// Two-way sliders (negative and positive values) show a "+" on positive values. The
    /// web's rule (`min < neutral`) also signs hues, radii and midpoints ("+220°"); here
    /// only sliders that go below zero are signed.
    pub fn two_way(&self) -> bool {
        self.min < 0.0
    }

    /// `v` on the step grid, inside the range, rounded to the shown decimals.
    pub fn snap(&self, v: f64) -> f64 {
        let stepped = self.min + ((v - self.min) / self.step).round() * self.step;
        let scale = 10f64.powi(self.decimals as i32);
        ((stepped.clamp(self.min, self.max)) * scale).round() / scale
    }

    /// The value as shown: `+0.35`, `-20`, `220°`.
    pub fn format(&self, v: f64) -> String {
        let sign = if self.two_way() && v > 0.0 { "+" } else { "" };
        format!("{sign}{v:.*}{}", self.decimals, self.unit)
    }
}

/// A coloured track instead of the plain one with a fill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Track {
    /// Blue to amber.
    Temperature,
    /// Green to magenta.
    Tint,
    /// The hue circle.
    Rainbow,
}

impl Track {
    fn stops(self) -> &'static [Color32] {
        const TEMP: [Color32; 3] = [
            Color32::from_rgb(0x3b, 0x82, 0xf6),
            Color32::from_rgb(0xe5, 0xe7, 0xeb),
            Color32::from_rgb(0xf5, 0x9e, 0x0b),
        ];
        const TINT: [Color32; 3] = [
            Color32::from_rgb(0x22, 0xc5, 0x5e),
            Color32::from_rgb(0xe5, 0xe7, 0xeb),
            Color32::from_rgb(0xd9, 0x46, 0xef),
        ];
        const RAINBOW: [Color32; 7] = [
            Color32::from_rgb(255, 0, 0),
            Color32::from_rgb(255, 255, 0),
            Color32::from_rgb(0, 255, 0),
            Color32::from_rgb(0, 255, 255),
            Color32::from_rgb(0, 0, 255),
            Color32::from_rgb(255, 0, 255),
            Color32::from_rgb(255, 0, 0),
        ];
        match self {
            Track::Temperature => &TEMP,
            Track::Tint => &TINT,
            Track::Rainbow => &RAINBOW,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SliderResponse {
    /// The value moved this frame (redraw the preview).
    pub changed: bool,
    /// The gesture ended with a change (commit once).
    pub released: bool,
}

impl SliderResponse {
    pub fn or(self, other: SliderResponse) -> SliderResponse {
        SliderResponse {
            changed: self.changed || other.changed,
            released: self.released || other.released,
        }
    }
}

/// How far a drag of `dx` points moves the value on a track `width` points wide: the whole
/// range across the track, a tenth of that with `fine` (`Shift`). `accum` is the unsnapped
/// value carried between frames, so slow fine drags still add up; returns the new `accum`
/// and the snapped value.
pub fn drag_step(accum: f64, dx: f64, width: f64, range: &Range, fine: bool) -> (f64, f64) {
    let per_point = (range.max - range.min) / width.max(1.0);
    let scale = if fine { 0.1 } else { 1.0 };
    let accum = (accum + dx * per_point * scale).clamp(range.min, range.max);
    (accum, range.snap(accum))
}

/// A double-click: back to neutral. A slider already at neutral records nothing.
pub fn reset(value: &mut f64, range: &Range) -> SliderResponse {
    let before = *value;
    *value = range.neutral;
    moved(before, *value)
}

/// An arrow key: one step up or down.
pub fn key_step(value: &mut f64, range: &Range, up: bool) -> SliderResponse {
    let before = *value;
    *value = range.snap(before + if up { range.step } else { -range.step });
    moved(before, *value)
}

/// Typed text: parsed (a leading `+` and the unit allowed), snapped and clamped.
pub fn typed(value: &mut f64, range: &Range, text: &str) -> SliderResponse {
    let text = text
        .trim()
        .trim_end_matches(range.unit)
        .trim()
        .trim_start_matches('+');
    match text.replace(',', ".").parse::<f64>() {
        Ok(v) if v.is_finite() => {
            let before = *value;
            *value = range.snap(v);
            moved(before, *value)
        }
        _ => SliderResponse::default(),
    }
}

fn moved(before: f64, after: f64) -> SliderResponse {
    let changed = before != after;
    SliderResponse {
        changed,
        released: changed,
    }
}

/// The row's height in points.
pub const ROW_HEIGHT: f32 = 34.0;

#[derive(Clone, Default)]
struct Memory {
    /// The unsnapped value while dragging.
    accum: Option<f64>,
    /// The value moved during this drag.
    moved: bool,
    /// The value text being typed.
    typing: Option<String>,
}

/// Draws one slider bound to `value` (see the module docs).
pub fn edit(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f64,
    range: Range,
    track: Option<Track>,
) -> SliderResponse {
    let id = ui.make_persistent_id(("slider", label));
    let width = ui.available_width().max(120.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, ROW_HEIGHT), Sense::hover());
    let mut memory: Memory = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    let mut out = SliderResponse::default();

    let font = FontId::proportional(12.5);
    let value_text = range.format(*value);
    let value_galley = ui
        .painter()
        .layout_no_wrap(value_text, font.clone(), theme::MUTED);
    let value_rect = Rect::from_min_size(
        pos2(rect.right() - value_galley.size().x - 4.0, rect.top() + 2.0),
        value_galley.size() + vec2(4.0, 2.0),
    );
    let track_rect = Rect::from_min_max(
        pos2(rect.left() + 6.0, rect.bottom() - 11.0),
        pos2(rect.right() - 6.0, rect.bottom() - 7.0),
    );

    let response = ui.interact(rect, id, Sense::click_and_drag());
    response.widget_info(|| WidgetInfo::slider(true, *value, label));

    if response.double_clicked() {
        memory.typing = None;
        out = out.or(reset(value, &range));
    } else if response.clicked()
        && memory.typing.is_none()
        && response
            .interact_pointer_pos()
            .is_some_and(|p| value_rect.expand(4.0).contains(p))
    {
        // Starts empty (the current value shows as the hint), so typing replaces it.
        memory.typing = Some(String::new());
    }

    if response.drag_started() {
        memory.accum = Some(*value);
        memory.moved = false;
    }
    if response.dragged()
        && let Some(accum) = memory.accum
    {
        let dx = f64::from(response.drag_delta().x);
        let fine = ui.input(|i| i.modifiers.shift);
        let (accum, v) = drag_step(accum, dx, f64::from(track_rect.width()), &range, fine);
        memory.accum = Some(accum);
        if v != *value {
            *value = v;
            memory.moved = true;
            out.changed = true;
        }
    }
    if response.drag_stopped() {
        out.released |= memory.moved;
        memory.accum = None;
        memory.moved = false;
    }

    if response.has_focus() && memory.typing.is_none() {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                id,
                egui::EventFilter {
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    ..Default::default()
                },
            );
        });
        let (up, down) = ui.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowRight)
                    || i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowLeft)
                    || i.consume_key(Modifiers::NONE, Key::ArrowDown),
            )
        });
        if up {
            out = out.or(key_step(value, &range, true));
        }
        if down {
            out = out.or(key_step(value, &range, false));
        }
    }

    if let Some(text) = memory.typing.as_mut() {
        // The value is being typed: a text field over the value (registered after the row,
        // so it takes the clicks inside it).
        let edit_rect = Rect::from_min_max(
            pos2(rect.right() - 64.0, rect.top()),
            pos2(rect.right(), rect.top() + 18.0),
        );
        let field = ui.put(
            edit_rect,
            TextEdit::singleline(text)
                .hint_text(format!("{:.*}", range.decimals, *value))
                .font(font.clone())
                .align(egui::Align2::RIGHT_CENTER),
        );
        let current = text.clone();
        field.widget_info(|| {
            let mut info = WidgetInfo::text_edit(true, &current, &current, "");
            info.label = Some(label.to_owned());
            info
        });
        if !field.has_focus() && !field.lost_focus() {
            field.request_focus();
        }
        let enter = ui.input(|i| i.key_pressed(Key::Enter));
        let escape = ui.input(|i| i.key_pressed(Key::Escape));
        if field.lost_focus() || enter || escape {
            if !escape {
                out = typed(value, &range, text);
            }
            memory.typing = None;
        }
    }

    // Paint: label, value, track.
    let painter = ui.painter();
    let label_colour = if response.hovered() || response.has_focus() {
        theme::INK
    } else {
        theme::INK.gamma_multiply(0.9)
    };
    painter.text(
        pos2(rect.left(), rect.top() + 2.0),
        egui::Align2::LEFT_TOP,
        label,
        font.clone(),
        label_colour,
    );
    if memory.typing.is_none() {
        let colour = if *value != range.neutral {
            theme::INK
        } else {
            theme::MUTED
        };
        painter.text(
            pos2(rect.right(), rect.top() + 2.0),
            egui::Align2::RIGHT_TOP,
            range.format(*value),
            font,
            colour,
        );
    }
    paint_track(painter, track_rect, *value, &range, track);
    if response.has_focus() {
        painter.rect_stroke(
            rect.expand(1.0),
            CornerRadius::same(3),
            Stroke::new(1.0, theme::BRAND),
            egui::StrokeKind::Outside,
        );
    }

    ui.data_mut(|d| d.insert_temp(id, memory));
    out
}

fn fraction(v: f64, range: &Range) -> f32 {
    (((v - range.min) / (range.max - range.min)).clamp(0.0, 1.0)) as f32
}

fn paint_track(
    painter: &egui::Painter,
    rect: Rect,
    value: f64,
    range: &Range,
    track: Option<Track>,
) {
    let x_at = |v: f64| rect.left() + rect.width() * fraction(v, range);
    match track {
        Some(track) => {
            let stops = track.stops();
            let n = stops.len() - 1;
            let mut mesh = egui::Mesh::default();
            for (i, colour) in stops.iter().enumerate() {
                let x = rect.left() + rect.width() * i as f32 / n as f32;
                mesh.colored_vertex(pos2(x, rect.top()), *colour);
                mesh.colored_vertex(pos2(x, rect.bottom()), *colour);
                if i > 0 {
                    let base = (i as u32 - 1) * 2;
                    mesh.add_triangle(base, base + 1, base + 2);
                    mesh.add_triangle(base + 1, base + 2, base + 3);
                }
            }
            painter.add(egui::Shape::mesh(mesh));
        }
        None => {
            painter.rect_filled(rect, CornerRadius::same(2), theme::LINE);
            let (a, b) = (
                x_at(value.min(range.neutral)),
                x_at(value.max(range.neutral)),
            );
            let fill = Rect::from_min_max(pos2(a, rect.top()), pos2(b, rect.bottom()));
            painter.rect_filled(fill, CornerRadius::same(2), theme::BRAND);
        }
    }
    let thumb = pos2(x_at(value), rect.center().y);
    painter.circle(thumb, 6.0, theme::SURFACE, Stroke::new(2.0, theme::BRAND));
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPOSURE: Range = Range::new(-5.0, 5.0).step(0.05, 2);

    #[test]
    fn slider_double_click_resets_to_neutral() {
        let mut v = 1.35;
        assert_eq!(
            reset(&mut v, &EXPOSURE),
            SliderResponse {
                changed: true,
                released: true
            }
        );
        assert_eq!(v, 0.0);
        // At neutral already: nothing to commit (as the web).
        assert_eq!(reset(&mut v, &EXPOSURE), SliderResponse::default());
        // A slider whose neutral is not zero goes back to its own neutral.
        let midpoint = Range::AMOUNT.neutral(50.0);
        let mut m = 80.0;
        assert!(reset(&mut m, &midpoint).released);
        assert_eq!(m, 50.0);
    }

    #[test]
    fn slider_shift_drag_is_finer() {
        // A 200-point track over −100..100: one point is one unit.
        let range = Range::BIPOLAR;
        let (_, normal) = drag_step(0.0, 10.0, 200.0, &range, false);
        assert_eq!(normal, 10.0);
        let (_, fine) = drag_step(0.0, 10.0, 200.0, &range, true);
        assert_eq!(fine, 1.0, "Shift moves a tenth as far");
        // Slow fine drags add up instead of snapping back each frame.
        let mut accum = 0.0;
        let mut v = 0.0;
        for _ in 0..3 {
            (accum, v) = drag_step(accum, 3.0, 200.0, &range, true);
        }
        assert_eq!(v, 1.0, "0.9 snaps to 1");
        // Clamped at the ends.
        let (_, end) = drag_step(90.0, 500.0, 200.0, &range, false);
        assert_eq!(end, 100.0);
        // Exposure keeps two decimals on its 0.05 grid.
        let (_, e) = drag_step(0.0, 7.0, 200.0, &EXPOSURE, false);
        assert_eq!(e, 0.35);
    }

    #[test]
    fn values_read_and_type_as_the_web() {
        assert_eq!(EXPOSURE.format(0.35), "+0.35");
        assert_eq!(Range::BIPOLAR.format(-20.0), "-20");
        assert_eq!(
            Range::new(0.0, 360.0)
                .neutral(220.0)
                .unit("°")
                .format(220.0),
            "220°"
        );
        assert_eq!(
            Range::AMOUNT.format(30.0),
            "30",
            "one-way sliders show no +"
        );
        let mut v = 0.0;
        assert!(typed(&mut v, &EXPOSURE, " +1.27 ").released);
        assert_eq!(v, 1.25, "snapped to the step");
        assert!(!typed(&mut v, &EXPOSURE, "lots").changed);
        assert!(key_step(&mut v, &EXPOSURE, true).released);
        assert_eq!(v, 1.3);
    }
}
