//! The Navigator (Develop's side panel, under the histogram): the whole edited photo, small,
//! with a frame round the part the preview shows when zoomed in. A click or drag on it moves
//! the preview there (as Lightroom's). Its canvas is drawn with the photo's current settings,
//! again only when they, the crop being drawn or the size change.

use super::{Develop, State, preview};
use crate::theme;
use crate::toast::Toasts;
use eframe::egui::{self, Color32, Sense, Stroke};
use lf_pipeline::renderer::{DrawOptions, View};

pub const TITLE: &str = "Navigator";
/// The tallest the navigator gets, in points.
const MAX_HEIGHT: f32 = 180.0;

/// Where `view` (fractions of the photo) sits inside `shown` (the navigator's picture).
pub fn frame_of(view: View, shown: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_size(
        shown.min
            + egui::vec2(
                view.x as f32 * shown.width(),
                view.y as f32 * shown.height(),
            ),
        egui::vec2(
            view.w as f32 * shown.width(),
            view.h as f32 * shown.height(),
        ),
    )
}

/// The photo point (fractions) under `pos` in `shown`, kept inside the photo.
pub fn point_at(pos: egui::Pos2, shown: egui::Rect) -> (f64, f64) {
    let t = (pos - shown.min) / shown.size();
    (
        f64::from(t.x.clamp(0.0, 1.0)),
        f64::from(t.y.clamp(0.0, 1.0)),
    )
}

/// What the navigator canvas was last drawn for.
#[derive(Clone, PartialEq)]
pub struct NavKey {
    size: (u32, u32),
    settings: lf_settings::EditSettings,
    crop: Option<lf_settings::Crop>,
}

impl Develop {
    /// The navigator section (open at first).
    pub(super) fn navigator_ui(&mut self, ui: &mut egui::Ui, toasts: &mut Toasts) {
        egui::CollapsingHeader::new(TITLE)
            .id_salt("navigator-section")
            .default_open(true)
            .show(ui, |ui| self.navigator_body(ui, toasts));
    }

    fn navigator_body(&mut self, ui: &mut egui::Ui, toasts: &mut Toasts) {
        let draw_crop = self.draw_crop();
        let (Some(render_state), State::Ready(photo, _)) =
            (self.render_state.clone(), &mut self.state)
        else {
            return;
        };
        let settings = self
            .session
            .edit()
            .map_or_else(|| photo.settings.clone(), |e| e.settings.clone());
        let mut geometry = lf_settings::geometry::Geometry::from(&settings);
        if let Some(crop) = draw_crop {
            geometry.crop = crop;
        }
        let r = &photo.renderer;
        let (ow, oh) = lf_settings::geometry::output_size(r.full_width, r.full_height, &geometry);
        let width = ui.available_width().max(1.0);
        let k = (width / ow.max(1) as f32).min(MAX_HEIGHT / oh.max(1) as f32);
        let size = egui::vec2(ow as f32 * k, oh as f32 * k);
        let (area, response) =
            ui.allocate_exact_size(egui::vec2(width, size.y), Sense::click_and_drag());
        let shown = egui::Rect::from_center_size(area.center(), size);
        let ppp = ui.ctx().pixels_per_point();
        let px = preview::canvas_size(&self.gpu, f64::from(size.x * ppp), f64::from(size.y * ppp));
        let canvas = self
            .navigator
            .get_or_insert_with(|| preview::Preview::new(render_state));
        let (target, texture, fresh) = match canvas.ensure(&self.gpu, px.0, px.1) {
            Ok(made) => made,
            Err(e) => {
                self.navigator = None;
                self.renderer_error("The navigator could not be made", e, toasts);
                return;
            }
        };
        let key = NavKey {
            size: px,
            settings,
            crop: draw_crop,
        };
        if fresh || self.navigator_drawn.as_ref() != Some(&key) {
            let opts = DrawOptions {
                crop: draw_crop,
                view: View::default(),
                ..DrawOptions::default()
            };
            if let Err(e) = photo.renderer.draw(&key.settings, &opts, target) {
                self.renderer_error("The navigator could not be drawn", e, toasts);
                return;
            }
            self.navigator_drawn = Some(key);
        }
        ui.painter().image(
            texture,
            shown,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        // The part the preview shows, when it is not the whole photo.
        let view = self.last_view;
        if view.w < 0.999 || view.h < 0.999 {
            let frame = frame_of(view, shown);
            ui.painter().rect_stroke(
                frame,
                0.0,
                Stroke::new(2.0, theme::INK),
                egui::StrokeKind::Inside,
            );
        }
        if (response.clicked() || response.dragged())
            && let Some(pos) = response.interact_pointer_pos()
        {
            self.viewport.centre = point_at(pos, shown);
            self.dirty = true;
        }
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, TITLE));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_view_frame_and_a_click_map_between_photo_and_navigator() {
        let shown = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(200.0, 100.0));
        let view = View {
            x: 0.25,
            y: 0.5,
            w: 0.5,
            h: 0.25,
        };
        let f = frame_of(view, shown);
        assert_eq!(f.min, egui::pos2(60.0, 70.0));
        assert_eq!(f.size(), egui::vec2(100.0, 25.0));
        assert_eq!(point_at(f.center(), shown), (0.5, 0.625));
        assert_eq!(
            point_at(egui::pos2(-50.0, 500.0), shown),
            (0.0, 1.0),
            "kept inside"
        );
    }
}
