//! View → Second window: a window of its own (for a second monitor) showing the photo being
//! worked on, fitted on black with its name and marks: the open photo in Develop, the active
//! one in the Library. It is rendered with its saved edits on the `Open` lane as Compare
//! renders ([`super::compare::render`]), again when another photo becomes the one shown and
//! when Develop's edits to it are saved (the thumbnail's quiet time, so not on every drag).
//! Closing the window, or the button again, turns it off.

use super::compare::{Pixels, place, render};
use super::{Env, Library, Mode};
use crate::jobs::{Lane, Msg};
use eframe::egui::{self, Color32, Vec2};

pub const TITLE: &str = "LensFlow Image Editor - Second window";
pub const NOTHING: &str = "No photo is selected.";

enum Picture {
    Waiting,
    Pending(Box<Pixels>),
    Shown(egui::TextureHandle),
    Failed(String),
}

/// The second window's photo and picture.
pub struct SecondWindow {
    /// The photo shown (or being rendered), if any.
    id: Option<i64>,
    picture: Picture,
    generation: u64,
    /// The shown photo's edits were saved since it was rendered.
    stale: bool,
}

impl SecondWindow {
    pub fn new() -> SecondWindow {
        SecondWindow {
            id: None,
            picture: Picture::Waiting,
            generation: 0,
            stale: false,
        }
    }

    /// Whether `wanted` needs rendering now.
    pub fn needs(&self, wanted: Option<i64>) -> bool {
        wanted.is_some() && (wanted != self.id || self.stale)
    }

    /// Notes that the edits of `ids` were saved.
    pub fn edited(&mut self, ids: &[i64]) {
        if self.id.is_some_and(|id| ids.contains(&id)) {
            self.stale = true;
        }
    }
}

impl Default for SecondWindow {
    fn default() -> Self {
        SecondWindow::new()
    }
}

impl Library {
    pub fn second_window_open(&self) -> bool {
        self.second_window.is_some()
    }

    /// The photo the second window has on screen (rendered), if any.
    pub fn second_window_shown(&self) -> Option<i64> {
        self.second_window
            .as_ref()
            .filter(|w| !matches!(w.picture, Picture::Waiting | Picture::Failed(_)))
            .and_then(|w| w.id)
    }

    pub(super) fn toggle_second_window(&mut self) {
        self.second_window = match self.second_window {
            Some(_) => None,
            None => Some(SecondWindow::new()),
        };
    }

    /// The photo the second window shows: Develop's, else the active one.
    fn second_window_photo(&self) -> Option<i64> {
        match self.mode {
            Mode::Develop(id) => Some(id),
            Mode::Library => self.selection.active(),
        }
    }

    /// Asks for a render when the photo to show changed or its edits were saved.
    pub(super) fn follow_second_window(&mut self, edited: &[i64], env: &mut Env) {
        let wanted = self.second_window_photo();
        let Some(window) = &mut self.second_window else {
            return;
        };
        window.edited(edited);
        if !window.needs(wanted) {
            return;
        }
        let Some(id) = wanted else {
            return;
        };
        let (Some(root), Some(path)) = (self.root.clone(), self.photo(id).map(|p| p.path.clone()))
        else {
            return;
        };
        self.compare_generation += 1;
        let generation = self.compare_generation;
        let Some(window) = &mut self.second_window else {
            return;
        };
        if window.id != Some(id) {
            window.picture = Picture::Waiting;
        }
        window.id = Some(id);
        window.generation = generation;
        window.stale = false;
        let quality = crate::develop::open::quality(env.prefs.develop_quality);
        let (gpu, cache) = (self.gpu.clone(), self.frames.clone());
        env.jobs.spawn(Lane::Open, "second window", move || {
            let result = render(&gpu, &root, id, &path, cache.as_deref(), quality)
                .map(|p| Box::new(Pixels(p)));
            Msg::SecondWindowRendered {
                generation,
                id,
                result,
            }
        });
    }

    pub(super) fn second_window_rendered(
        &mut self,
        generation: u64,
        id: i64,
        result: Result<Box<Pixels>, String>,
    ) {
        if let Some(w) = self
            .second_window
            .as_mut()
            .filter(|w| w.generation == generation && w.id == Some(id))
        {
            w.picture = match result {
                Ok(p) => Picture::Pending(p),
                Err(e) => Picture::Failed(e),
            };
        }
    }

    /// The second window itself (its own OS window where the platform allows).
    pub(super) fn second_window_ui(&mut self, ctx: &egui::Context) {
        let caption = self
            .second_window
            .as_ref()
            .and_then(|w| w.id)
            .and_then(|id| self.photo(id))
            .map(|row| {
                let name = crate::develop::open::file_name(&row.path);
                match super::cull_ui::describe(row) {
                    Some(marks) => format!("{name}  ·  {marks}"),
                    None => name,
                }
            });
        let Some(w) = &mut self.second_window else {
            return;
        };
        if let Picture::Pending(_) = w.picture
            && let Picture::Pending(p) = std::mem::replace(&mut w.picture, Picture::Waiting)
        {
            let img = &p.0;
            let pixels = egui::ColorImage::from_rgba_unmultiplied(
                [img.width() as usize, img.height() as usize],
                img.as_raw(),
            );
            w.picture = Picture::Shown(ctx.load_texture(
                "second-window",
                pixels,
                egui::TextureOptions::LINEAR,
            ));
        }
        let builder = egui::ViewportBuilder::default()
            .with_title(TITLE)
            .with_inner_size([960.0, 640.0]);
        let closed = ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("second-window"),
            builder,
            |ui, _class| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(Color32::BLACK))
                    .show(ui, |ui| {
                        let area = ui.available_rect_before_wrap();
                        let picture =
                            egui::Rect::from_min_max(area.min, area.max - Vec2::new(0.0, 28.0));
                        let painter = ui.painter_at(area);
                        let note = |text: &str| {
                            painter.text(
                                picture.center(),
                                egui::Align2::CENTER_CENTER,
                                text,
                                egui::FontId::proportional(15.0),
                                Color32::GRAY,
                            );
                        };
                        match (&w.picture, w.id) {
                            (_, None) => note(NOTHING),
                            (Picture::Shown(t), _) => {
                                let (screen, uv, _) =
                                    place(picture, t.size_vec2(), 1.0, Vec2::splat(0.5));
                                painter.image(t.id(), screen, uv, Color32::WHITE);
                            }
                            (Picture::Failed(e), _) => note(e),
                            _ => note("Rendering…"),
                        }
                        if let Some(caption) = &caption {
                            painter.text(
                                egui::pos2(area.center().x, area.bottom() - 14.0),
                                egui::Align2::CENTER_CENTER,
                                caption,
                                egui::FontId::proportional(13.0),
                                Color32::from_gray(170),
                            );
                        }
                        ui.allocate_rect(area, egui::Sense::hover());
                    });
                ui.ctx().input(|i| i.viewport().close_requested())
            },
        );
        if closed {
            self.second_window = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_renders_a_new_photo_and_an_edited_one_again() {
        let mut w = SecondWindow::new();
        assert!(!w.needs(None), "nothing to show");
        assert!(w.needs(Some(3)));
        w.id = Some(3);
        assert!(!w.needs(Some(3)), "already shown");
        w.edited(&[7]);
        assert!(!w.needs(Some(3)), "another photo's edits");
        w.edited(&[3]);
        assert!(w.needs(Some(3)), "its edits were saved");
        assert!(w.needs(Some(4)), "another photo");
    }
}
