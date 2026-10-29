//! Reference view (`Shift+R`, View → Reference): in Develop, another photo shown beside the one
//! being edited, to match its look by eye. The reference is the photo chosen with the
//! filmstrip's "Use as reference" (right-click), else the photo open before this one. It is
//! rendered with its saved edits on the `Open` lane as Compare renders
//! ([`super::compare::render`]) and stays put while other photos are opened beside it.

use super::compare::{Pixels, place, render};
use super::{Env, Library, Mode};
use crate::jobs::{Lane, Msg};
use crate::theme;
use eframe::egui::UiBuilder;
use eframe::egui::{self, Color32, Rect, RichText, Vec2};

pub const NO_REFERENCE: &str =
    "Right-click a photo in the filmstrip and choose \"Use as reference\" first.";
pub const USE_AS_REFERENCE: &str = "Use as reference";
pub const REFERENCE: &str = "Reference";
/// The space between the reference and the photo being edited, in points.
const GAP: f32 = 8.0;

enum Picture {
    Waiting,
    Pending(Box<Pixels>),
    Shown(egui::TextureHandle),
    Failed(String),
}

/// The reference photo and its picture.
pub struct Reference {
    pub id: i64,
    picture: Picture,
    generation: u64,
}

/// The reference's half and the edited photo's half of `area`, side by side.
pub fn split(area: Rect) -> (Rect, Rect) {
    let w = ((area.width() - GAP) / 2.0).max(0.0);
    let left = Rect::from_min_size(area.min, Vec2::new(w, area.height()));
    let right = Rect::from_min_max(egui::pos2(area.max.x - w, area.min.y), area.max);
    (left, right)
}

impl Library {
    pub fn reference_shown(&self) -> bool {
        self.reference_on && matches!(self.mode, Mode::Develop(_))
    }

    /// The photo to show as the reference: the one chosen, else the one open before.
    fn reference_choice(&self) -> Option<i64> {
        self.reference_pick
            .or(self.previous_developed)
            .filter(|id| self.photo(*id).is_some())
    }

    /// `Shift+R`: shows or hides the reference beside the photo being edited.
    pub(super) fn toggle_reference(&mut self, env: &mut Env) {
        if self.reference_on {
            self.reference_on = false;
            return;
        }
        let Some(id) = self.reference_choice() else {
            env.toasts.push(crate::toast::Level::Info, NO_REFERENCE);
            return;
        };
        self.reference_on = true;
        self.load_reference(id, env);
    }

    /// The filmstrip's "Use as reference": `id` becomes the reference (and shows).
    pub(super) fn use_as_reference(&mut self, id: i64, env: &mut Env) {
        self.reference_pick = Some(id);
        self.reference_on = true;
        self.load_reference(id, env);
    }

    /// Renders `id` for the reference pane, unless it is already the one shown.
    fn load_reference(&mut self, id: i64, env: &mut Env) {
        if self.reference.as_ref().is_some_and(|r| r.id == id) {
            return;
        }
        let (Some(root), Some(path)) = (self.root.clone(), self.photo(id).map(|p| p.path.clone()))
        else {
            return;
        };
        self.compare_generation += 1;
        let generation = self.compare_generation;
        let quality = crate::develop::open::quality(env.prefs.develop_quality);
        let (gpu, cache) = (self.gpu.clone(), self.frames.clone());
        env.jobs.spawn(Lane::Open, "reference", move || {
            let result = render(&gpu, &root, id, &path, cache.as_deref(), quality)
                .map(|p| Box::new(Pixels(p)));
            Msg::ReferenceRendered {
                generation,
                id,
                result,
            }
        });
        self.reference = Some(Reference {
            id,
            picture: Picture::Waiting,
            generation,
        });
    }

    pub(super) fn reference_rendered(
        &mut self,
        generation: u64,
        id: i64,
        result: Result<Box<Pixels>, String>,
    ) {
        if let Some(r) = self
            .reference
            .as_mut()
            .filter(|r| r.generation == generation && r.id == id)
        {
            r.picture = match result {
                Ok(p) => Picture::Pending(p),
                Err(e) => Picture::Failed(e),
            };
        }
    }

    /// The reference, fitted in `rect`, with its name above.
    pub(super) fn reference_ui(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let name = self
            .reference
            .as_ref()
            .and_then(|r| self.photo(r.id))
            .map(|p| crate::develop::open::file_name(&p.path))
            .unwrap_or_default();
        let Some(r) = &mut self.reference else {
            return;
        };
        if let Picture::Pending(_) = r.picture
            && let Picture::Pending(p) = std::mem::replace(&mut r.picture, Picture::Waiting)
        {
            let img = &p.0;
            let pixels = egui::ColorImage::from_rgba_unmultiplied(
                [img.width() as usize, img.height() as usize],
                img.as_raw(),
            );
            r.picture = Picture::Shown(ui.ctx().load_texture(
                format!("reference-{}", r.id),
                pixels,
                egui::TextureOptions::LINEAR,
            ));
        }
        ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
            ui.label(RichText::new(format!("{REFERENCE}: {name}")).color(theme::MUTED));
            let picture = ui.available_rect_before_wrap();
            let painter = ui.painter_at(picture);
            match &r.picture {
                Picture::Shown(t) => {
                    let (screen, uv, _) = place(picture, t.size_vec2(), 1.0, Vec2::splat(0.5));
                    painter.image(t.id(), screen, uv, Color32::WHITE);
                }
                Picture::Failed(e) => {
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
            let response = ui.allocate_rect(picture, egui::Sense::hover());
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Image, true, format!("{REFERENCE} {name}"))
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_area_is_split_in_two_halves_with_a_gap() {
        let area = Rect::from_min_size(egui::pos2(10.0, 20.0), Vec2::new(808.0, 400.0));
        let (l, r) = split(area);
        assert_eq!(l.width(), 400.0);
        assert_eq!(r.width(), 400.0);
        assert_eq!(l.min, area.min);
        assert_eq!(r.max, area.max);
        assert_eq!(r.min.x - l.max.x, GAP);
    }
}
