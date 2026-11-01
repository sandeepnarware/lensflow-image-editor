//! Develop → "Match a Lightroom example…": choose the original photo and Lightroom's export of
//! it with the preset applied, match ([`crate::lightroom::example_job`] on the `Long` lane;
//! the pickers run on their own threads), then see our render before, Lightroom's, ours
//! matched and the difference, how close it came in plain words, and save the result over
//! the preset matched or as a new one. A weak match is said so but can still be saved: the
//! user may know the export was cropped and accept it.

use crate::lightroom::example_job::{Previews, Verdict};
use crate::lightroom::import::shown_name;
use crate::theme;
use eframe::egui::{self, Key, RichText};
use lf_grade::example::Fit;
use lf_settings::lightroom::budget::desktop_fits;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub const TITLE: &str = "Match a Lightroom example";
pub const ABOUT: &str = "Give a photo and Lightroom's export of it with the preset applied. LensFlow learns the colour, vignette and grain that make its own render look like Lightroom's, and keeps them in a preset.";
pub const GUIDANCE: &str = "Export from Lightroom at full size (or at least 2000 px), in sRGB, without cropping, straightening or lens corrections if you can.";
pub const ORIGINAL: &str = "Original photo";
pub const EXPORT: &str = "Lightroom's export";
pub const CHOOSE_ORIGINAL: &str = "Choose photo…";
pub const CHOOSE_EXPORT: &str = "Choose export…";
pub const NOT_CHOSEN: &str = "Not chosen yet.";
pub const NEW_PRESET: &str = "The result becomes a new preset.";
pub const MATCH: &str = "Match";
pub const CANCEL: &str = "Cancel";
pub const MATCHING: &str = "Matching: rendering the photo and learning Lightroom's look…";
pub const STOP: &str = "Stop";
pub const CLOSE: &str = "Close";
pub const SAVE: &str = "Save";
pub const SAVE_AS_NEW: &str = "Save as new";
pub const BEFORE: &str = "Ours before";
pub const LIGHTROOM: &str = "Lightroom";
pub const AFTER: &str = "Ours matched";
pub const DIFFERENCE: &str = "Difference (x4)";
pub const TOO_LARGE: &str = "The matched preset is too large to keep.";
/// The difference's unit (ΔE2000).
pub const DELTA_E: &str = "ΔE";

/// The files the export picker offers.
pub const EXPORT_FILES: &[&str] = &["jpg", "jpeg", "tif", "tiff", "png"];

/// "Matching the preset “Moody”."
pub fn matching_text(name: &str) -> String {
    format!("Matching the preset “{name}”.")
}

/// The new preset's name before numbering: "Moody (matched)".
pub fn new_name(name: &str) -> String {
    format!("{name} (matched)")
}

/// "Difference: ΔE 1.6 average, 3.9 at most for 95 % of pixels."
pub fn difference_text(fit: &Fit) -> String {
    format!(
        "Difference: {DELTA_E} {:.1} average, {:.1} at most for 95 % of pixels.",
        fit.delta_e_mean, fit.delta_e_p95
    )
}

/// The details line: what decided whether the pair lined up.
pub fn details_text(fit: &Fit) -> String {
    let lined = if fit.aligned {
        "lined up"
    } else {
        "did not line up"
    };
    format!("Alignment residual {:.3} ({lined}).", fit.residual)
}

/// What a match gave: the preset to save, the fit and the pictures.
#[derive(Debug, Clone, PartialEq)]
pub struct Matched {
    pub preset: Value,
    pub fit: Fit,
    pub previews: Previews,
}

/// Where the dialog is.
#[derive(Debug, Clone, PartialEq)]
pub enum Stage {
    Choosing,
    Matching,
    Done(Box<Matched>),
    /// The match could not be made: why.
    Failed(String),
}

/// Which file a picker chooses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    Original,
    Export,
}

/// What the dialog asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LrExampleAction {
    None,
    Choose(Pick),
    /// Match the chosen pair.
    Match,
    /// Save over the preset matched.
    Save,
    /// Save as a new preset.
    SaveAsNew,
    Close,
}

static IDS: AtomicU64 = AtomicU64::new(1);

pub struct LrExampleDialog {
    /// This dialog's own id: an earlier dialog's answers are dropped.
    pub id: u64,
    /// The preset matched (its name and settings), or none for a new preset.
    pub preset: Option<(String, Value)>,
    pub original: Option<PathBuf>,
    pub export: Option<PathBuf>,
    pub stage: Stage,
    pub stop: Arc<AtomicBool>,
    /// Why the last save failed, shown under the buttons.
    pub error: Option<String>,
    /// The previews on the device, made on the first frame that shows them.
    textures: Option<[egui::TextureHandle; 4]>,
}

impl LrExampleDialog {
    pub fn new(preset: Option<(String, Value)>, original: Option<PathBuf>) -> LrExampleDialog {
        LrExampleDialog {
            id: IDS.fetch_add(1, Ordering::Relaxed),
            preset,
            original,
            export: None,
            stage: Stage::Choosing,
            stop: Arc::new(AtomicBool::new(false)),
            error: None,
            textures: None,
        }
    }

    /// The name a new preset starts from: the preset's own, else the export's file name.
    pub fn base_name(&self) -> String {
        match &self.preset {
            Some((name, _)) => name.clone(),
            None => self
                .export
                .as_ref()
                .and_then(|p| p.file_stem())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Lightroom example".to_owned()),
        }
    }

    /// A match ended: its result is shown.
    pub fn finished(&mut self, result: Result<Box<Matched>, String>) {
        self.textures = None;
        self.stage = match result {
            Ok(m) => Stage::Done(m),
            Err(e) => Stage::Failed(e),
        };
    }

    pub fn show(&mut self, ctx: &egui::Context) -> LrExampleAction {
        let mut action = LrExampleAction::None;
        let _ = egui::Modal::new(egui::Id::unique("lr-example")).show(ctx, |ui| {
            ui.set_width(640.0);
            ui.heading(TITLE);
            ui.add_space(8.0);
            match &self.stage {
                Stage::Choosing => action = self.choosing_ui(ui),
                Stage::Matching => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(MATCHING);
                    });
                    ui.add_space(8.0);
                    let stopping = self.stop.load(Ordering::SeqCst);
                    if ui.add_enabled(!stopping, egui::Button::new(STOP)).clicked() {
                        self.stop.store(true, Ordering::SeqCst);
                    }
                }
                Stage::Done(_) => action = self.done_ui(ui),
                Stage::Failed(why) => {
                    ui.label(RichText::new(shown_name(ui.ctx(), why)).color(theme::DANGER));
                    ui.add_space(12.0);
                    if ui.button(CLOSE).clicked() {
                        action = LrExampleAction::Close;
                    }
                }
            }
        });
        let running = self.stage == Stage::Matching;
        if !running && action == LrExampleAction::None && ctx.input(|i| i.key_pressed(Key::Escape))
        {
            action = LrExampleAction::Close;
        }
        action
    }

    fn choosing_ui(&mut self, ui: &mut egui::Ui) -> LrExampleAction {
        let mut action = LrExampleAction::None;
        ui.label(RichText::new(ABOUT).color(theme::MUTED));
        ui.add_space(4.0);
        ui.label(RichText::new(GUIDANCE).color(theme::MUTED).small());
        ui.add_space(8.0);
        match &self.preset {
            Some((name, _)) => ui.label(matching_text(&shown_name(ui.ctx(), name))),
            None => ui.label(NEW_PRESET),
        };
        ui.add_space(8.0);
        let rows = [
            (ORIGINAL, CHOOSE_ORIGINAL, Pick::Original, &self.original),
            (EXPORT, CHOOSE_EXPORT, Pick::Export, &self.export),
        ];
        egui::Grid::new("lr-example-files")
            .num_columns(3)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                for (label, button, pick, path) in rows {
                    ui.label(label);
                    if ui.button(button).clicked() {
                        action = LrExampleAction::Choose(pick);
                    }
                    let text = path.as_ref().map_or(NOT_CHOSEN.to_owned(), |p| {
                        shown_name(ui.ctx(), &p.display().to_string())
                    });
                    ui.add(egui::Label::new(RichText::new(text).small()).truncate());
                    ui.end_row();
                }
            });
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let can = self.original.is_some() && self.export.is_some();
            if ui.add_enabled(can, egui::Button::new(MATCH)).clicked() {
                action = LrExampleAction::Match;
            }
            if ui.button(CANCEL).clicked() {
                action = LrExampleAction::Close;
            }
        });
        action
    }

    fn done_ui(&mut self, ui: &mut egui::Ui) -> LrExampleAction {
        let Stage::Done(matched) = &self.stage else {
            return LrExampleAction::None;
        };
        let mut action = LrExampleAction::None;
        let textures = self.textures.get_or_insert_with(|| {
            let p = &matched.previews;
            let load = |name: &str, img: &egui::ColorImage| {
                ui.ctx()
                    .load_texture(name, img.clone(), egui::TextureOptions::LINEAR)
            };
            [
                load("lr-example-before", &p.before),
                load("lr-example-lightroom", &p.lightroom),
                load("lr-example-after", &p.after),
                load("lr-example-difference", &p.difference),
            ]
        });
        let captions = [BEFORE, LIGHTROOM, AFTER, DIFFERENCE];
        let cell = (ui.available_width() - 12.0) / 2.0;
        egui::Grid::new("lr-example-previews")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                for row in 0..2 {
                    for col in 0..2 {
                        let i = row * 2 + col;
                        let t = &textures[i];
                        let [w, h] = t.size().map(|v| v as f32);
                        let k = (cell / w.max(1.0)).min(200.0 / h.max(1.0));
                        ui.vertical(|ui| {
                            ui.add(egui::Image::new(egui::load::SizedTexture::new(
                                t.id(),
                                egui::vec2(w * k, h * k),
                            )));
                            ui.label(RichText::new(captions[i]).small().color(theme::MUTED));
                        });
                    }
                    ui.end_row();
                }
            });
        ui.add_space(8.0);
        let fit = &matched.fit;
        let verdict = Verdict::of(fit);
        let colour = match verdict {
            Verdict::VeryClose => theme::SUCCESS,
            Verdict::Close => ui.visuals().text_color(),
            Verdict::NotAGoodMatch => theme::WARNING,
        };
        ui.label(RichText::new(verdict.text()).strong().color(colour));
        ui.label(difference_text(fit));
        ui.label(RichText::new(details_text(fit)).small().color(theme::MUTED));
        ui.add_space(12.0);
        let fits = desktop_fits(&matched.preset);
        ui.horizontal(|ui| {
            if self.preset.is_some() {
                let save = ui
                    .add_enabled(fits, egui::Button::new(SAVE))
                    .on_disabled_hover_text(TOO_LARGE);
                if save.clicked() {
                    action = LrExampleAction::Save;
                }
            }
            let new = ui
                .add_enabled(fits, egui::Button::new(SAVE_AS_NEW))
                .on_disabled_hover_text(TOO_LARGE);
            if new.clicked() {
                action = LrExampleAction::SaveAsNew;
            }
            if ui.button(CANCEL).clicked() {
                action = LrExampleAction::Close;
            }
        });
        if !fits {
            ui.label(RichText::new(TOO_LARGE).color(theme::DANGER).small());
        }
        if let Some(e) = &self.error {
            ui.label(
                RichText::new(shown_name(ui.ctx(), e))
                    .color(theme::DANGER)
                    .small(),
            );
        }
        action
    }
}
