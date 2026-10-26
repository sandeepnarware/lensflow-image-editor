//! The histogram panel: R, G and B filled curves plus luma, recomputed at most once per change
//! and only after the photo has been still for [`DELAY`].

use crate::theme;
use eframe::egui::{self, Color32, Pos2, Sense, Shape, Stroke};
use lf_pipeline::renderer::Histogram;
use std::time::{Duration, Instant};

/// The panel's size in points.
pub const WIDTH: f32 = 256.0;
pub const HEIGHT: f32 = 100.0;

/// How long the settings must be still before the histogram is remade.
pub const DELAY: Duration = Duration::from_millis(200);

/// When to remake the histogram (Review Focus 1: never on every frame of a drag).
pub struct Throttle;

impl Throttle {
    /// True when `DELAY` has passed since the last change and the histogram has not been made
    /// since that change.
    pub fn should_run(last_change: Instant, last_run: Option<Instant>, now: Instant) -> bool {
        now.saturating_duration_since(last_change) >= DELAY
            && last_run.is_none_or(|run| run < last_change)
    }
}

/// Draws `histogram` (or an empty panel while it is being made): R, G and B as translucent
/// filled curves, luma as a line, scaled to the tallest bin away from the two ends (so a
/// clipped spike does not flatten the rest).
pub fn show(ui: &mut egui::Ui, histogram: Option<&Histogram>) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(WIDTH, HEIGHT), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, theme::SURFACE_2);
    let Some(h) = histogram else {
        return;
    };
    let tallest = [&h.r, &h.g, &h.b, &h.l]
        .iter()
        .flat_map(|bins| bins[1..255].iter())
        .copied()
        .max()
        .unwrap_or(0)
        .max(1) as f32;
    let point = |i: usize, v: u32| {
        let x = rect.left() + rect.width() * i as f32 / 255.0;
        let y = rect.bottom() - rect.height() * (v as f32 / tallest).min(1.0);
        Pos2::new(x, y)
    };
    let channels = [
        (&h.r, Color32::from_rgba_unmultiplied(239, 68, 68, 90)),
        (&h.g, Color32::from_rgba_unmultiplied(74, 222, 128, 90)),
        (&h.b, Color32::from_rgba_unmultiplied(96, 165, 250, 90)),
    ];
    for (bins, colour) in channels {
        // One thin quad per bin: filled curves without a concave polygon.
        let mut mesh = egui::Mesh::default();
        for i in 0..255 {
            let (a, b) = (point(i, bins[i]), point(i + 1, bins[i + 1]));
            let base = mesh.vertices.len() as u32;
            mesh.colored_vertex(Pos2::new(a.x, rect.bottom()), colour);
            mesh.colored_vertex(a, colour);
            mesh.colored_vertex(b, colour);
            mesh.colored_vertex(Pos2::new(b.x, rect.bottom()), colour);
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base, base + 2, base + 3);
        }
        painter.add(Shape::mesh(mesh));
    }
    let luma: Vec<Pos2> = (0..256).map(|i| point(i, h.l[i])).collect();
    painter.add(Shape::line(luma, Stroke::new(1.0, theme::INK)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throttle_runs_once_200ms_after_last_change() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        // Still dragging: changes every 16 ms, never 200 ms apart.
        let mut last_run = None;
        let mut runs = 0;
        let mut last_change = t0;
        for frame in 0..60u64 {
            let now = ms(frame * 16);
            last_change = now;
            if Throttle::should_run(last_change, last_run, now) {
                runs += 1;
                last_run = Some(now);
            }
        }
        assert_eq!(runs, 0, "no histogram while the slider moves");
        let stop = last_change;
        let at = |n: u64| stop + Duration::from_millis(n);
        assert!(!Throttle::should_run(stop, last_run, at(199)));
        assert!(Throttle::should_run(stop, last_run, at(200)));
        last_run = Some(at(200));
        // Once per change: not again until something changes.
        assert!(!Throttle::should_run(stop, last_run, at(400)));
        assert!(!Throttle::should_run(stop, last_run, at(5_000)));
        let changed = at(6_000);
        assert!(!Throttle::should_run(changed, last_run, at(6_100)));
        assert!(Throttle::should_run(changed, last_run, at(6_200)));
    }
}
