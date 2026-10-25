//! The web app's dark palette applied to egui.

use eframe::egui::{self, Color32};

pub const BG: Color32 = Color32::from_rgb(0x0f, 0x0f, 0x12);
pub const SURFACE: Color32 = Color32::from_rgb(0x18, 0x18, 0x1c);
pub const SURFACE_2: Color32 = Color32::from_rgb(0x22, 0x22, 0x28);
pub const LINE: Color32 = Color32::from_rgb(0x2e, 0x2e, 0x36);
pub const INK: Color32 = Color32::from_rgb(0xf4, 0xf4, 0xf6);
pub const MUTED: Color32 = Color32::from_rgb(0xa0, 0xa0, 0xab);
pub const BRAND: Color32 = Color32::from_rgb(0x6d, 0x28, 0xd9);
pub const BRAND_CONTRAST: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);
pub const DANGER: Color32 = Color32::from_rgb(0xf8, 0x71, 0x71);
pub const SUCCESS: Color32 = Color32::from_rgb(0x4a, 0xde, 0x80);
pub const WARNING: Color32 = Color32::from_rgb(0xfb, 0xbf, 0x24);

/// The LensFlow logo, 512 px (see `assets/README.md`); also the window icon.
pub const LOGO_PNG: &[u8] = include_bytes!("../../../assets/logo.png");

/// Switches `ctx` to dark mode with the LensFlow palette.
pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.set_visuals_of(egui::Theme::Dark, visuals());
}

fn visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    v.window_fill = BG;
    v.extreme_bg_color = BG;
    v.panel_fill = SURFACE;
    v.faint_bg_color = SURFACE_2;
    v.code_bg_color = SURFACE_2;
    v.window_stroke.color = LINE;
    v.hyperlink_color = BRAND;
    v.weak_text_color = Some(MUTED);
    v.warn_fg_color = WARNING;
    v.error_fg_color = DANGER;
    v.selection.bg_fill = BRAND;
    v.selection.stroke.color = BRAND_CONTRAST;

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = SURFACE;
    w.noninteractive.weak_bg_fill = SURFACE;
    w.noninteractive.bg_stroke.color = LINE;
    w.noninteractive.fg_stroke.color = INK;
    w.inactive.bg_fill = SURFACE_2;
    w.inactive.weak_bg_fill = SURFACE_2;
    w.inactive.fg_stroke.color = INK;
    w.hovered.bg_fill = LINE;
    w.hovered.weak_bg_fill = LINE;
    w.hovered.bg_stroke.color = MUTED;
    w.hovered.fg_stroke.color = INK;
    w.active.bg_fill = BRAND;
    w.active.weak_bg_fill = BRAND;
    w.active.bg_stroke.color = BRAND;
    w.active.fg_stroke.color = BRAND_CONTRAST;
    w.open.bg_fill = SURFACE_2;
    w.open.weak_bg_fill = SURFACE_2;
    w.open.bg_stroke.color = LINE;
    w.open.fg_stroke.color = INK;
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(c: Color32) -> String {
        format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
    }

    #[test]
    fn theme_palette_matches_web() {
        let ctx = egui::Context::default();
        apply(&ctx);
        let visuals = ctx.global_style().visuals.clone();
        assert!(visuals.dark_mode);
        assert_eq!(hex(visuals.window_fill), "#0f0f12", "window fill is bg");
        assert_eq!(hex(visuals.panel_fill), "#18181c", "panel fill is surface");
        assert_eq!(
            hex(visuals.faint_bg_color),
            "#222228",
            "faint fill is surface-2"
        );
        assert_eq!(
            hex(visuals.window_stroke.color),
            "#2e2e36",
            "window stroke is line"
        );
        assert_eq!(hex(visuals.text_color()), "#f4f4f6", "text is ink");
        assert_eq!(
            hex(visuals.weak_text_color()),
            "#a0a0ab",
            "weak text is muted"
        );
        assert_eq!(hex(visuals.hyperlink_color), "#6d28d9", "links are brand");
    }
}
