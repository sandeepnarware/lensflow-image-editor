//! The adjust panels: the web's `AdjustPanel.vue` sections in its order and with its labels
//! (Light, Colour, Colour mix, Tone curve, Colour grading, Detail, Effects), each folding
//! and with a "Reset" link, plus a RAW section for RAW photos. The panels change the
//! settings they are given and say what happened; the Develop view commits, resets and
//! redraws.

pub mod calibration;
pub mod color;
pub mod crop;
pub mod curve;
pub mod detail;
pub mod effects;
pub mod grading;
pub mod hsl;
pub mod lens;
pub mod light;
pub mod looks;
pub mod lut;
pub mod negative;
pub mod presets;
pub mod raw;
pub mod slider;
pub mod versions;

use crate::develop::auto::Sample;
use crate::theme;
use eframe::egui::{self, RichText};
use lf_pipeline::renderer::Histogram;
use lf_settings::EditSettings;
use lf_settings::groups::GroupKey;
use slider::SliderResponse;

/// One folding section: its label (the web's), the setting groups its "Reset" puts back, and
/// whether it starts open.
pub struct Section {
    pub key: &'static str,
    pub label: &'static str,
    pub groups: &'static [GroupKey],
    pub open: bool,
}

/// The RAW section's label (shown for RAW photos only, above the rest).
pub const RAW: &str = "RAW settings";

/// The web's sections, in its order.
pub const SECTIONS: [Section; 7] = [
    Section {
        key: "light",
        label: "Light",
        groups: &[GroupKey::Light],
        open: true,
    },
    Section {
        key: "color",
        label: "Colour",
        groups: &[GroupKey::Color, GroupKey::Bw],
        open: true,
    },
    Section {
        key: "hsl",
        label: "Colour mix",
        groups: &[GroupKey::Hsl],
        open: false,
    },
    Section {
        key: "curve",
        label: "Tone curve",
        groups: &[GroupKey::Curve],
        open: false,
    },
    Section {
        key: "grading",
        label: "Colour grading",
        groups: &[GroupKey::Grading],
        open: false,
    },
    Section {
        key: "detail",
        label: "Detail",
        groups: &[GroupKey::Detail, GroupKey::Sharpen],
        open: false,
    },
    Section {
        key: "effects",
        label: "Effects",
        groups: &[GroupKey::Effects],
        open: false,
    },
];

/// The panels' own state between frames (the sections' folding lives in egui's memory).
#[derive(Debug, Clone, Default)]
pub struct PanelState {
    pub hsl_mode: hsl::Mode,
    pub curve: curve::CurveState,
}

/// What the panels did this frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PanelOutput {
    /// The settings changed (redraw the preview).
    pub changed: bool,
    /// A gesture ended: commit (one history step).
    pub commit: bool,
    /// A section's "Reset" was clicked: put these groups back (one history step).
    pub reset: Option<&'static [GroupKey]>,
    /// The white-balance eyedropper was clicked: open (or leave) its tool.
    pub pick_wb: bool,
    /// "Import .cube…" was clicked.
    pub import_lut: bool,
    /// "Import .dcp…" was clicked.
    pub import_dcp: bool,
    /// "Apply lens profile" was clicked.
    pub lens_profile: bool,
}

impl PanelOutput {
    fn absorb(&mut self, r: SliderResponse) {
        self.changed |= r.changed;
        self.commit |= r.released;
    }
}

/// What the panels need to know about the photo.
pub struct PanelContext<'a> {
    /// A RAW photo (shows the RAW section).
    pub raw: bool,
    /// Behind the tone curve.
    pub histogram: Option<&'a Histogram>,
    /// The photo, small, for Auto tone and Auto white balance (none: their buttons are off).
    pub sample: Option<&'a Sample>,
    /// The white-balance eyedropper is open (its button shows pressed).
    pub picking_wb: bool,
    /// The LUT looks loaded (`lf_pipeline::lut::names`).
    pub luts: &'a [String],
    /// The camera profiles loaded (`lf_raw::dcp::names`: name, camera).
    pub profiles: &'a [(String, String)],
    /// The RAW's as-shot colour temperature (kelvin), when its headers say.
    pub as_shot_kelvin: Option<f64>,
}

/// A folding section with a "Reset" link; true when the link was clicked.
fn section(
    ui: &mut egui::Ui,
    key: &str,
    label: &str,
    open: bool,
    body: impl FnOnce(&mut egui::Ui),
) -> bool {
    let id = ui.make_persistent_id(("section", key));
    let state =
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, open);
    let mut reset = false;
    let mut toggle = false;
    let mut header = state.show_header(ui, |ui| {
        // The whole label folds and unfolds the section, as the web's header button does.
        let title = egui::Button::new(RichText::new(label).strong().color(theme::INK)).frame(false);
        if ui.add(title).clicked() {
            toggle = true;
        }
        {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let link = egui::Button::new(RichText::new("Reset").small().color(theme::MUTED))
                    .frame(false);
                let response = ui.add(link).on_hover_text(format!("Reset {label}"));
                // Named for what it resets (as the web's aria-label).
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Reset {label}"))
                });
                if response.clicked() {
                    reset = true;
                }
            });
        }
    });
    if toggle {
        header.toggle();
    }
    let (_, _, _) = header.body(body);
    ui.separator();
    reset
}

/// Draws every section for `s`.
pub fn show(
    ui: &mut egui::Ui,
    state: &mut PanelState,
    s: &mut EditSettings,
    ctx: &PanelContext,
) -> PanelOutput {
    let mut out = PanelOutput::default();
    if ctx.raw {
        let mut r = SliderResponse::default();
        if section(ui, "raw", RAW, true, |ui| {
            let (rr, import) = raw::show(ui, s, ctx.profiles);
            r = rr;
            out.import_dcp |= import;
        }) {
            let before = s.clone();
            raw::reset(s);
            if *s != before {
                r = SliderResponse {
                    changed: true,
                    released: true,
                };
            }
        }
        out.absorb(r);
    }
    for sec in &SECTIONS {
        let mut r = SliderResponse::default();
        let reset = section(ui, sec.key, sec.label, sec.open, |ui| {
            r = match sec.key {
                "light" => light::show(ui, s, ctx.sample),
                "color" => {
                    let (r, pick) =
                        color::show(ui, s, ctx.sample, ctx.picking_wb, ctx.as_shot_kelvin);
                    out.pick_wb |= pick;
                    r
                }
                "hsl" => hsl::show(ui, &mut state.hsl_mode, s),
                "curve" => {
                    let c = curve::edit(ui, &mut state.curve, &mut s.curve, ctx.histogram);
                    SliderResponse {
                        changed: c.changed,
                        released: c.released,
                    }
                }
                "grading" => grading::show(ui, s),
                "detail" => detail::show(ui, s),
                _ => effects::show(ui, s),
            };
        });
        out.absorb(r);
        if reset {
            out.reset = Some(sec.groups);
        }
    }
    // Desktop only, after the web's sections: its Reset is its own, as the RAW section's.
    let mut r = SliderResponse::default();
    let mut profile = false;
    if section(ui, "lens", lens::TITLE, false, |ui| {
        r = lens::show(ui, s, ctx.sample, &mut profile)
    }) {
        let before = s.clone();
        lens::reset(s);
        if *s != before {
            r = SliderResponse {
                changed: true,
                released: true,
            };
        }
    }
    out.absorb(r);
    out.lens_profile |= profile;
    let mut r = SliderResponse::default();
    if section(ui, "negative", negative::TITLE, false, |ui| {
        r = negative::show(ui, s, ctx.sample)
    }) {
        let before = s.clone();
        negative::reset(s);
        if *s != before {
            r = SliderResponse {
                changed: true,
                released: true,
            };
        }
    }
    out.absorb(r);
    let mut r = SliderResponse::default();
    if section(ui, "calibration", calibration::TITLE, false, |ui| {
        r = calibration::show(ui, s)
    }) {
        let before = s.clone();
        calibration::reset(s);
        if *s != before {
            r = SliderResponse {
                changed: true,
                released: true,
            };
        }
    }
    out.absorb(r);
    let mut r = SliderResponse::default();
    if section(ui, "lut", lut::TITLE, false, |ui| {
        let (lr, import) = lut::show(ui, s, ctx.luts);
        r = lr;
        out.import_lut |= import;
    }) {
        let before = s.clone();
        lut::reset(s);
        if *s != before {
            r = SliderResponse {
                changed: true,
                released: true,
            };
        }
    }
    out.absorb(r);
    out
}
