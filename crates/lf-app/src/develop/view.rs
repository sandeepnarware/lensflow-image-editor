//! Zoom and pan: which part of the output the preview shows, and where on screen.
//!
//! Sizes are `(width, height)` in pixels: `output` is the edited photo's full-resolution
//! output size, `panel` the preview area in physical pixels. A scale of 1.0 shows one output
//! pixel per screen pixel ("100 %"). The centre is the output point (as fractions) at the
//! middle of the panel; it is clamped so the photo never leaves the panel, and the photo is
//! centred on an axis where it is smaller than the panel.

use lf_pipeline::renderer::View;

/// Smallest and largest scale the wheel reaches.
pub const MIN_SCALE: f64 = 0.02;
pub const MAX_SCALE: f64 = 16.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Zoom {
    /// The whole photo fits the panel.
    Fit,
    /// Output pixels per screen pixel.
    Scale(f64),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    pub zoom: Zoom,
    /// The output point at the panel's centre, as fractions of the output.
    pub centre: (f64, f64),
}

impl Default for Viewport {
    fn default() -> Viewport {
        Viewport {
            zoom: Zoom::Fit,
            centre: (0.5, 0.5),
        }
    }
}

/// Where the photo lands: the part of the output shown, and the on-screen rectangle (in panel
/// pixels from the panel's top left) it is drawn into.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub view: View,
    /// `(x, y, width, height)` inside the panel.
    pub rect: (f64, f64, f64, f64),
}

/// The scale at which `output` just fits `panel`, never above 1.0: a photo smaller than the
/// panel shows at actual size (as Lightroom), not enlarged.
pub fn fit(output_w: f64, output_h: f64, panel_w: f64, panel_h: f64) -> f64 {
    if output_w <= 0.0 || output_h <= 0.0 {
        return 1.0;
    }
    (panel_w / output_w)
        .min(panel_h / output_h)
        .clamp(f64::MIN_POSITIVE, 1.0)
}

/// The centre on one axis: clamped so the photo covers the panel, or 0.5 (centred) when the
/// photo (`shown` screen pixels long) is shorter than the panel.
fn clamp_centre(c: f64, shown: f64, panel: f64) -> f64 {
    if shown <= panel {
        0.5
    } else {
        let half = panel / shown / 2.0;
        c.clamp(half, 1.0 - half)
    }
}

/// One axis of a placement: `(view offset, view length, rect offset, rect length)`.
fn axis(c: f64, shown: f64, panel: f64) -> (f64, f64, f64, f64) {
    if shown <= panel {
        (0.0, 1.0, (panel - shown) / 2.0, shown)
    } else {
        let len = panel / shown;
        let c = clamp_centre(c, shown, panel);
        (c - len / 2.0, len, 0.0, panel)
    }
}

impl Viewport {
    /// The current scale (output pixels per screen pixel).
    pub fn scale(&self, output: (f64, f64), panel: (f64, f64)) -> f64 {
        match self.zoom {
            Zoom::Fit => fit(output.0, output.1, panel.0, panel.1),
            Zoom::Scale(k) => k,
        }
    }

    /// The centre actually used (clamped, or centred on a short axis).
    fn effective_centre(&self, output: (f64, f64), panel: (f64, f64)) -> (f64, f64) {
        let s = self.scale(output, panel);
        (
            clamp_centre(self.centre.0, output.0 * s, panel.0),
            clamp_centre(self.centre.1, output.1 * s, panel.1),
        )
    }

    /// The part of the output shown, as fractions (clamped to the photo).
    pub fn view_rect(&self, output: (f64, f64), panel: (f64, f64)) -> View {
        self.placement(output, panel).view
    }

    /// The view and the on-screen rectangle together.
    pub fn placement(&self, output: (f64, f64), panel: (f64, f64)) -> Placement {
        let s = self.scale(output, panel);
        let (vx, vw, rx, rw) = axis(self.centre.0, output.0 * s, panel.0);
        let (vy, vh, ry, rh) = axis(self.centre.1, output.1 * s, panel.1);
        Placement {
            view: View {
                x: vx,
                y: vy,
                w: vw,
                h: vh,
            },
            rect: (rx, ry, rw, rh),
        }
    }

    /// The output point (fractions) under `cursor` (fractions of the panel).
    pub fn output_at(
        &self,
        cursor: (f64, f64),
        output: (f64, f64),
        panel: (f64, f64),
    ) -> (f64, f64) {
        let s = self.scale(output, panel);
        let (cx, cy) = self.effective_centre(output, panel);
        let at = |cursor: f64, c: f64, o: f64, p: f64| c + (cursor - 0.5) * p / (o * s);
        (
            at(cursor.0, cx, output.0, panel.0),
            at(cursor.1, cy, output.1, panel.1),
        )
    }

    /// Zooms by `factor` keeping the output point under `cursor` (fractions of the panel)
    /// still. Reaching the fit scale or below shows the whole photo (`Fit`).
    pub fn zoom_at(
        &mut self,
        factor: f64,
        cursor: (f64, f64),
        output: (f64, f64),
        panel: (f64, f64),
    ) {
        let point = self.output_at(cursor, output, panel);
        let fitted = fit(output.0, output.1, panel.0, panel.1);
        let scale = (self.scale(output, panel) * factor).clamp(MIN_SCALE, MAX_SCALE);
        if scale <= fitted * (1.0 + 1e-9) {
            *self = Viewport::default();
            return;
        }
        self.zoom = Zoom::Scale(scale);
        let centre = |pt: f64, cursor: f64, o: f64, p: f64| {
            let shown = o * scale;
            clamp_centre(pt - (cursor - 0.5) * p / shown, shown, p)
        };
        self.centre = (
            centre(point.0, cursor.0, output.0, panel.0),
            centre(point.1, cursor.1, output.1, panel.1),
        );
    }

    /// Drags the photo by `(dx, dy)` screen pixels.
    pub fn pan(&mut self, dx: f64, dy: f64, output: (f64, f64), panel: (f64, f64)) {
        let s = self.scale(output, panel);
        let (cx, cy) = self.effective_centre(output, panel);
        let (w, h) = (output.0 * s, output.1 * s);
        self.centre = (
            clamp_centre(cx - dx / w, w, panel.0),
            clamp_centre(cy - dy / h, h, panel.1),
        );
    }

    /// `Fit` ⇄ 100 %. A photo that fits at 100 % already stays at `Fit`.
    pub fn toggle_fit_100(&mut self, output: (f64, f64), panel: (f64, f64)) {
        self.zoom = match self.zoom {
            Zoom::Fit if fit(output.0, output.1, panel.0, panel.1) < 1.0 => Zoom::Scale(1.0),
            _ => Zoom::Fit,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn fit_scales_to_panel_both_orientations() {
        // Landscape photo in a landscape panel: the width binds.
        assert!(close(fit(6000.0, 4000.0, 1200.0, 900.0), 0.2));
        // Portrait photo in the same panel: the height binds.
        assert!(close(fit(4000.0, 6000.0, 1200.0, 900.0), 0.15));
        // A small photo is never enlarged: it fits at 100 % (as Lightroom).
        assert!(close(fit(300.0, 200.0, 1200.0, 900.0), 1.0));
        let p = Viewport::default().placement((300.0, 200.0), (1200.0, 900.0));
        assert_eq!(p.rect, (450.0, 350.0, 300.0, 200.0), "centred, actual size");

        let v = Viewport::default();
        let p = v.placement((4000.0, 6000.0), (1200.0, 900.0));
        assert_eq!(p.view, View::default(), "fit shows the whole photo");
        // 600×900 centred horizontally.
        let (x, y, w, h) = p.rect;
        assert!(close(x, 300.0) && close(y, 0.0) && close(w, 600.0) && close(h, 900.0));
    }

    #[test]
    fn view_rect_clamps_at_edges() {
        let output = (4000.0, 3000.0);
        let panel = (1000.0, 750.0);
        let mut v = Viewport {
            zoom: Zoom::Scale(1.0),
            centre: (0.0, 1.0),
        };
        let r = v.view_rect(output, panel);
        assert!(close(r.x, 0.0) && close(r.y, 0.75), "{r:?}");
        assert!(close(r.w, 0.25) && close(r.h, 0.25), "{r:?}");

        // Dragging past the edge stops at it.
        v.pan(5000.0, -5000.0, output, panel);
        let r = v.view_rect(output, panel);
        assert!(close(r.x, 0.0) && close(r.y, 0.75), "{r:?}");
        v.pan(-1e6, 1e6, output, panel);
        let r = v.view_rect(output, panel);
        assert!(close(r.x, 0.75) && close(r.y, 0.0), "{r:?}");

        // A photo narrower than the panel stays centred on that axis.
        let tall = Viewport {
            zoom: Zoom::Scale(0.2),
            centre: (0.9, 0.1),
        };
        let p = tall.placement((4000.0, 6000.0), (1000.0, 750.0));
        assert!(close(p.view.x, 0.0) && close(p.view.w, 1.0), "{p:?}");
        assert!(close(p.rect.0, 100.0) && close(p.rect.2, 800.0), "{p:?}");
        assert!(close(p.view.y, 0.0), "clamped to the top: {p:?}");
    }

    #[test]
    fn zoom_at_keeps_cursor_point() {
        let output = (4000.0, 3000.0);
        let panel = (1000.0, 750.0);
        let cursor = (0.25, 0.75);
        let mut v = Viewport {
            zoom: Zoom::Scale(1.0),
            centre: (0.5, 0.5),
        };
        let before = v.output_at(cursor, output, panel);
        v.zoom_at(2.0, cursor, output, panel);
        assert_eq!(v.zoom, Zoom::Scale(2.0));
        let after = v.output_at(cursor, output, panel);
        assert!(
            close(before.0, after.0) && close(before.1, after.1),
            "{before:?} -> {after:?}"
        );

        // From fit, too (fit is 0.25 here).
        let mut v = Viewport::default();
        let before = v.output_at(cursor, output, panel);
        v.zoom_at(4.0, cursor, output, panel);
        assert_eq!(v.zoom, Zoom::Scale(1.0));
        let after = v.output_at(cursor, output, panel);
        assert!(
            close(before.0, after.0) && close(before.1, after.1),
            "{before:?} -> {after:?}"
        );

        // Zooming out past fit shows the whole photo.
        v.zoom_at(0.1, cursor, output, panel);
        assert_eq!(v.zoom, Zoom::Fit);
        assert_eq!(v.view_rect(output, panel), View::default());
    }

    #[test]
    fn toggle_between_fit_and_one() {
        let (output, panel) = ((4000.0, 3000.0), (1000.0, 750.0));
        let mut v = Viewport::default();
        v.toggle_fit_100(output, panel);
        assert_eq!(v.zoom, Zoom::Scale(1.0));
        v.toggle_fit_100(output, panel);
        assert_eq!(v.zoom, Zoom::Fit);
        v.zoom = Zoom::Scale(3.0);
        v.toggle_fit_100(output, panel);
        assert_eq!(v.zoom, Zoom::Fit);

        // A photo smaller than the panel fits at 100 % already: nothing to toggle.
        let small = (300.0, 200.0);
        let mut v = Viewport::default();
        v.toggle_fit_100(small, panel);
        assert_eq!(v.zoom, Zoom::Fit);
        assert!(close(v.scale(small, panel), 1.0));
        // Zoomed in on it, the toggle goes back to fit.
        v.zoom = Zoom::Scale(4.0);
        v.toggle_fit_100(small, panel);
        assert_eq!(v.zoom, Zoom::Fit);
    }
}
