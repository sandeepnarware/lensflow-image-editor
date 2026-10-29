//! AI → Remove objects: the photo (rendered with its edits, [`crate::library::compare::render`])
//! fills the dialog and a brush paints over what should go: a stray person, a wire, a sign.
//! "Remove" hands the painted mask to the AI tool ([`crate::ai_job::AiTool::Remove`]), which
//! fills it from its surroundings and saves a new photo beside the original.
//!
//! The mask is kept at most [`MASK_EDGE`] pixels on its long side (removal feathers its edge
//! anyway), so painting stays quick and the red overlay is cheap to redraw.
//!
//! "Select people" paints every person the people model finds (widened a little, so their
//! edges and shadows' start go too); rubbing out whoever should stay, then Remove, takes the
//! rest away (Lightroom's distraction removal of people). "Select wires" paints the wires and
//! power lines found against the sky ([`lf_ai::wires`]) the same way.

use crate::theme;
use eframe::egui::{self, Color32, Key, Pos2, Rect, RichText, Sense, Vec2};
use lf_ai::Gray;

pub const TITLE: &str = "Remove objects";
pub const HINT: &str = "Paint over what should go. Alt-drag rubs out.";
pub const REMOVE: &str = "Remove";
pub const CLEAR: &str = "Clear";
pub const CANCEL: &str = "Cancel";
pub const SIZE: &str = "Brush size";
pub const RENDERING: &str = "Rendering the photo…";
pub const SELECT_PEOPLE: &str = "Select people";
pub const SELECTING: &str = "Finding the people…";
pub const NO_PEOPLE: &str = "No people were found.";
pub const PEOPLE_HINT: &str = "Rub out (Alt-drag) whoever should stay, then Remove.";
pub const SELECT_WIRES: &str = "Select wires";
pub const SELECTING_WIRES: &str = "Finding wires against the sky…";
pub const NO_WIRES: &str = "No wires were found against the sky.";
pub const WIRES_HINT: &str = "Rub out (Alt-drag) anything that is not a wire, then Remove.";
/// How much a found wire is widened, as a share of the mask's long edge.
const WIDEN_WIRES: f32 = 0.003;
/// How much a found person is widened, as a share of the mask's long edge.
const WIDEN: f32 = 0.008;

/// The mask's long edge, in pixels.
pub const MASK_EDGE: usize = 1024;

#[derive(Debug, Clone, PartialEq)]
pub enum RemoveAction {
    None,
    /// Remove what this mask (the photo's shape) paints.
    Remove(Gray),
    /// Find the people in this picture (the mask's size) and paint them.
    SelectPeople(lf_ai::Rgb),
    /// Find the wires against the sky in this picture (the mask's size) and paint them.
    SelectWires(lf_ai::Rgb),
    Close,
}

pub struct RemoveDialog {
    pub id: i64,
    /// The photo, once rendered; its size.
    texture: Option<egui::TextureHandle>,
    pending: Option<image::RgbaImage>,
    failed: Option<String>,
    /// The painted mask (0 or 255), sized to the photo's shape.
    mask: Vec<u8>,
    mw: usize,
    mh: usize,
    overlay: Option<egui::TextureHandle>,
    dirty: bool,
    /// The brush's radius, as a fraction of the mask's long edge.
    pub size: f32,
    last: Option<Pos2>,
    /// The photo at the mask's size (what "Select people" looks at).
    picture: Option<lf_ai::Rgb>,
    /// "Select people" or "Select wires" is running (which: `wires`); what it last said.
    pub selecting: bool,
    selecting_wires: bool,
    note: Option<&'static str>,
}

impl RemoveDialog {
    pub fn new(id: i64) -> RemoveDialog {
        RemoveDialog {
            id,
            texture: None,
            pending: None,
            failed: None,
            mask: Vec::new(),
            mw: 0,
            mh: 0,
            overlay: None,
            dirty: false,
            size: 0.02,
            last: None,
            picture: None,
            selecting: false,
            selecting_wires: false,
            note: None,
        }
    }

    /// "Select people" answered: everyone found is painted (added to what is painted).
    pub fn people_found(&mut self, result: Result<Gray, String>) {
        self.found(result, WIDEN, NO_PEOPLE, PEOPLE_HINT);
    }

    /// "Select wires" answered: the wires found are painted (added to what is painted).
    pub fn wires_found(&mut self, result: Result<Gray, String>) {
        self.found(result, WIDEN_WIRES, NO_WIRES, WIRES_HINT);
    }

    /// What a selection found, widened by `widen` of the long edge and painted.
    fn found(
        &mut self,
        result: Result<Gray, String>,
        widen_by: f32,
        none: &'static str,
        hint: &'static str,
    ) {
        self.selecting = false;
        self.selecting_wires = false;
        match result {
            Ok(found) if found.w == self.mw && found.h == self.mh => {
                let people: Vec<bool> = found.data.iter().map(|&v| v >= 0.5).collect();
                if !people.contains(&true) {
                    self.note = Some(none);
                    return;
                }
                let r = (widen_by * self.mw.max(self.mh) as f32).ceil() as usize;
                for (m, on) in self
                    .mask
                    .iter_mut()
                    .zip(widen(&people, self.mw, self.mh, r))
                {
                    if on {
                        *m = 255;
                    }
                }
                self.dirty = true;
                self.note = Some(hint);
            }
            Ok(_) => self.note = Some(none),
            Err(e) => log::warn!("selecting failed: {e}"),
        }
    }

    /// The rendered photo arrived (or could not be made).
    pub fn rendered(&mut self, result: Result<image::RgbaImage, String>) {
        match result {
            Ok(img) => {
                let long = img.width().max(img.height()).max(1) as f64;
                let k = (MASK_EDGE as f64 / long).min(1.0);
                self.mw = ((f64::from(img.width()) * k).round() as usize).max(1);
                self.mh = ((f64::from(img.height()) * k).round() as usize).max(1);
                self.mask = vec![0; self.mw * self.mh];
                self.picture = Some(lf_ai::Rgb::from_rgba8(&img).resized(self.mw, self.mh));
                self.pending = Some(img);
            }
            Err(e) => self.failed = Some(e),
        }
    }

    /// Whether anything is painted.
    pub fn painted(&self) -> bool {
        self.mask.iter().any(|&v| v > 0)
    }

    /// Paints (or rubs out) a dab at mask pixel `(x, y)`.
    pub fn dab(&mut self, x: f32, y: f32, erase: bool) {
        let r = (self.size * self.mw.max(self.mh) as f32).max(1.0);
        let (x0, x1) = (
            (x - r).floor().max(0.0) as usize,
            ((x + r).ceil() as usize).min(self.mw),
        );
        let (y0, y1) = (
            (y - r).floor().max(0.0) as usize,
            ((y + r).ceil() as usize).min(self.mh),
        );
        for py in y0..y1 {
            for px in x0..x1 {
                if (px as f32 + 0.5 - x).hypot(py as f32 + 0.5 - y) <= r {
                    self.mask[py * self.mw + px] = if erase { 0 } else { 255 };
                }
            }
        }
        self.dirty = true;
    }

    /// Dabs along the line from `a` to `b` (mask pixels), close enough to join up.
    fn line(&mut self, a: Pos2, b: Pos2, erase: bool) {
        let step = (self.size * self.mw.max(self.mh) as f32 * 0.4).max(1.0);
        let n = ((b - a).length() / step).ceil().max(1.0) as usize;
        for i in 0..=n {
            let p = a + (b - a) * (i as f32 / n as f32);
            self.dab(p.x, p.y, erase);
        }
    }

    pub fn mask(&self) -> Gray {
        Gray {
            w: self.mw,
            h: self.mh,
            data: self.mask.iter().map(|&v| f32::from(v) / 255.0).collect(),
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> RemoveAction {
        if let Some(img) = self.pending.take() {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [img.width() as usize, img.height() as usize],
                img.as_raw(),
            );
            self.texture =
                Some(ctx.load_texture("remove-photo", image, egui::TextureOptions::LINEAR));
        }
        if self.dirty && self.mw > 0 {
            let pixels: Vec<Color32> = self
                .mask
                .iter()
                .map(|&v| {
                    if v > 0 {
                        Color32::from_rgba_unmultiplied(230, 40, 40, 140)
                    } else {
                        Color32::TRANSPARENT
                    }
                })
                .collect();
            let image = egui::ColorImage::new([self.mw, self.mh], pixels);
            self.overlay =
                Some(ctx.load_texture("remove-mask", image, egui::TextureOptions::LINEAR));
            self.dirty = false;
        }
        let mut action = RemoveAction::None;
        let screen = ctx.content_rect();
        let _ = egui::Modal::new(egui::Id::unique("remove-dialog")).show(ctx, |ui| {
            let max = Vec2::new(screen.width() * 0.8, screen.height() * 0.7);
            ui.set_width(max.x.min(1100.0));
            ui.heading(TITLE);
            ui.label(RichText::new(HINT).color(theme::MUTED));
            ui.add_space(6.0);
            match (&self.texture, &self.failed) {
                (_, Some(e)) => {
                    ui.label(RichText::new(e).color(theme::WARNING));
                }
                (None, None) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RENDERING);
                    });
                }
                (Some(texture), None) => {
                    let size = texture.size_vec2();
                    let room = Vec2::new(ui.available_width(), max.y.min(720.0));
                    let k = (room.x / size.x).min(room.y / size.y);
                    let (rect, response) = ui.allocate_exact_size(size * k, Sense::drag());
                    let id = texture.id();
                    ui.painter().image(id, rect, unit(), Color32::WHITE);
                    if let Some(overlay) = &self.overlay {
                        ui.painter()
                            .image(overlay.id(), rect, unit(), Color32::WHITE);
                    }
                    let erase = ui.input(|i| i.modifiers.alt);
                    let to_mask = |p: Pos2| {
                        Pos2::new(
                            (p.x - rect.left()) / rect.width() * self.mw as f32,
                            (p.y - rect.top()) / rect.height() * self.mh as f32,
                        )
                    };
                    if let Some(p) = response.interact_pointer_pos()
                        && (response.dragged() || response.drag_started())
                    {
                        let m = to_mask(p);
                        match self.last {
                            Some(last) => self.line(last, m, erase),
                            None => self.dab(m.x, m.y, erase),
                        }
                        self.last = Some(m);
                    }
                    if response.drag_stopped() {
                        self.last = None;
                    }
                    if let Some(p) = response.hover_pos() {
                        let r = self.size * self.mw.max(self.mh) as f32 * rect.width()
                            / self.mw.max(1) as f32;
                        ui.painter()
                            .circle_stroke(p, r, egui::Stroke::new(1.5, Color32::WHITE));
                    }
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::Image, true, "Photo to paint on")
                    });
                }
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let label = ui.label(SIZE);
                let mut pct = self.size * 100.0;
                ui.add(egui::Slider::new(&mut pct, 0.3..=10.0).suffix(" %"))
                    .labelled_by(label.id);
                self.size = pct / 100.0;
                if ui.button(CLEAR).clicked() {
                    self.mask.fill(0);
                    self.dirty = true;
                    self.note = None;
                }
                let ready = self.picture.is_some() && !self.selecting;
                if ui
                    .add_enabled(ready, egui::Button::new(SELECT_PEOPLE))
                    .clicked()
                    && let Some(picture) = &self.picture
                {
                    self.selecting = true;
                    self.note = None;
                    action = RemoveAction::SelectPeople(picture.clone());
                }
                if ui
                    .add_enabled(ready, egui::Button::new(SELECT_WIRES))
                    .clicked()
                    && let Some(picture) = &self.picture
                {
                    self.selecting = true;
                    self.selecting_wires = true;
                    self.note = None;
                    action = RemoveAction::SelectWires(picture.clone());
                }
            });
            if self.selecting {
                let doing = if self.selecting_wires {
                    SELECTING_WIRES
                } else {
                    SELECTING
                };
                ui.label(RichText::new(doing).color(theme::MUTED));
            } else if let Some(note) = self.note {
                ui.label(RichText::new(note).color(theme::MUTED));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(self.painted(), egui::Button::new(REMOVE))
                    .clicked()
                {
                    action = RemoveAction::Remove(self.mask());
                }
                if ui.button(CANCEL).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                    action = RemoveAction::Close;
                }
            });
        });
        action
    }
}

/// `on` (a `w × h` grid) grown by `r` pixels: every pixel within `r` of one that is on.
fn widen(on: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    // Rows then columns, each through the distance to the nearest pixel that is on (a square
    // grow, quick at any radius).
    let pass = |src: &[bool], horizontal: bool| -> Vec<bool> {
        let mut out = vec![false; w * h];
        let (len, lines) = if horizontal { (w, h) } else { (h, w) };
        let at = |line: usize, i: usize| {
            if horizontal {
                line * w + i
            } else {
                i * w + line
            }
        };
        for line in 0..lines {
            let mut last: Option<usize> = None;
            let mut near = vec![usize::MAX; len];
            for i in 0..len {
                if src[at(line, i)] {
                    last = Some(i);
                }
                if let Some(l) = last {
                    near[i] = i - l;
                }
            }
            last = None;
            for i in (0..len).rev() {
                if src[at(line, i)] {
                    last = Some(i);
                }
                if let Some(l) = last {
                    near[i] = near[i].min(l - i);
                }
                out[at(line, i)] = near[i] <= r;
            }
        }
        out
    };
    pass(&pass(on, true), false)
}

fn unit() -> Rect {
    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn people_found_are_painted_widened_and_added() {
        let mut d = RemoveDialog::new(1);
        d.rendered(Ok(image::RgbaImage::new(100, 50)));
        assert!(d.picture.is_some());
        d.dab(10.0, 10.0, false);
        let mut found = Gray {
            w: 100,
            h: 50,
            data: vec![0.0; 5000],
        };
        found.data[25 * 100 + 50] = 0.9;
        d.people_found(Ok(found));
        assert_eq!(d.mask[10 * 100 + 10], 255, "what was painted stays");
        assert_eq!(d.mask[25 * 100 + 50], 255);
        assert_eq!(d.mask[25 * 100 + 51], 255, "widened");
        assert_eq!(d.mask[25 * 100 + 60], 0);
        assert_eq!(d.note, Some(PEOPLE_HINT));
        d.people_found(Ok(Gray {
            w: 100,
            h: 50,
            data: vec![0.1; 5000],
        }));
        assert_eq!(d.note, Some(NO_PEOPLE));
    }

    #[test]
    fn wires_found_are_painted_and_none_says_so() {
        let mut d = RemoveDialog::new(1);
        d.rendered(Ok(image::RgbaImage::new(100, 50)));
        let mut found = Gray {
            w: 100,
            h: 50,
            data: vec![0.0; 5000],
        };
        for x in 0..100 {
            found.data[20 * 100 + x] = 1.0;
        }
        d.wires_found(Ok(found));
        assert_eq!(d.mask[20 * 100 + 40], 255);
        assert_eq!(d.note, Some(WIRES_HINT));
        d.wires_found(Ok(Gray {
            w: 100,
            h: 50,
            data: vec![0.0; 5000],
        }));
        assert_eq!(d.note, Some(NO_WIRES));
    }

    #[test]
    fn widening_reaches_r_pixels_each_way() {
        let (w, h) = (9, 9);
        let mut on = vec![false; w * h];
        on[4 * w + 4] = true;
        let out = widen(&on, w, h, 2);
        assert!(out[4 * w + 6] && out[2 * w + 4] && out[6 * w + 6]);
        assert!(!out[4 * w + 7] && !out[w + 4]);
    }

    #[test]
    fn painting_and_rubbing_out_change_the_mask() {
        let mut d = RemoveDialog::new(1);
        d.rendered(Ok(image::RgbaImage::new(2048, 1024)));
        assert_eq!((d.mw, d.mh), (1024, 512));
        assert!(!d.painted());
        d.line(Pos2::new(100.0, 100.0), Pos2::new(300.0, 100.0), false);
        assert!(d.painted());
        assert_eq!(d.mask[100 * 1024 + 200], 255, "the line joins up");
        d.dab(200.0, 100.0, true);
        assert_eq!(d.mask[100 * 1024 + 200], 0);
        let m = d.mask();
        assert_eq!((m.w, m.h), (1024, 512));
        assert_eq!(m.data[100 * 1024 + 120], 1.0);
    }
}
