//! "Export N photos" (`Ctrl+Shift+E`): format, size, file name pattern, metadata, replace,
//! destination folder. The form is remembered in `prefs.last_export` as the `ExportSpec`
//! JSON. The work is [`crate::export_job::run`] on the `Long` lane; while it runs the dialog
//! shows "12 of 40" and Stop, and closing the window leaves the export running (the status
//! bar keeps the count and a toast tells how it ended).

use crate::catalog_ext::count;
use crate::export_job::{ExportOutcome, Failed};
use crate::theme;
use eframe::egui::{self, Key, RichText};
use lf_catalog::colour::OutputSpace;
use lf_catalog::export::{Destination, ExportSpec, Format, Metadata, Pattern, Size};
use lf_catalog::finish::{Corner, Sharpen, Stamp, Watermark};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub const EXPORT: &str = "Export";
pub const STOP: &str = "Stop";
pub const CLOSE: &str = "Close";
pub const CANCEL: &str = "Cancel";
pub const DONE: &str = "Done";
pub const SHOW_FOLDER: &str = "Show folder";
pub const REPLACE: &str = "Replace existing files";
pub const CHOOSE: &str = "Choose a folder…";
pub const WAITING: &str = "Waiting for the folder picker…";
pub const TYPE_PATH: &str = "Or type a folder path";
pub const USE_FOLDER: &str = "Use this folder";
pub const TOKENS: &str = "Tokens: {name} {seq} {date} {camera} {title}";
pub const KEEPS_RUNNING: &str = "Closing this window does not stop the export.";
/// Validation: shown under the Export button.
pub const NO_FOLDER: &str = "Choose a folder to export to.";
pub const NO_PATTERN: &str = "Enter a file name pattern.";
pub const BAD_SIZE: &str = "Enter a size larger than 0.";
/// Under the Percent field above 100.
pub const ENLARGES: &str = "Above 100 % enlarges the photo.";
/// Output sharpening, the watermark and their problems.
pub const SHARPEN: &str = "Sharpen for screen";
pub const WATERMARK: &str = "Add a watermark";
pub const WATERMARK_TEXT: &str = "Watermark text";
pub const CHOOSE_LOGO: &str = "Choose a logo…";
pub const COLOUR_SPACE: &str = "Colour space";
pub const CHOOSE_PROFILE: &str = "Profile…";
pub const NO_WATERMARK_TEXT: &str = "Type the watermark's text, or turn the watermark off.";
pub const NO_LOGO: &str = "Choose a logo picture, or turn the watermark off.";
/// The watermark's text to start from.
pub const DEFAULT_STAMP: &str = "©";
/// The default JPEG quality and file name pattern.
pub const DEFAULT_QUALITY: u8 = 93;
pub const DEFAULT_PATTERN: &str = "{name}";

/// "Export 3 photos".
pub fn title(n: usize) -> String {
    if n == 1 {
        "Export 1 photo".to_owned()
    } else {
        format!("Export {} photos", count(n))
    }
}

/// "12 of 40".
pub fn progress_text(done: usize, total: usize) -> String {
    format!("{} of {}", count(done), count(total))
}

/// "Exporting 12 of 40" for the status bar.
pub fn status_text(done: usize, total: usize) -> String {
    format!("Exporting {}", progress_text(done, total))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatKind {
    Jpeg,
    Png,
    WebP,
    Avif,
    Tiff16,
}

/// A watermark of text or of a logo picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StampKind {
    Text,
    Logo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeKind {
    Original,
    LongEdge,
    ShortEdge,
    Megapixels,
    Percent,
}

impl SizeKind {
    pub const ALL: [SizeKind; 5] = [
        SizeKind::Original,
        SizeKind::LongEdge,
        SizeKind::ShortEdge,
        SizeKind::Megapixels,
        SizeKind::Percent,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SizeKind::Original => "Original",
            SizeKind::LongEdge => "Long edge (px)",
            SizeKind::ShortEdge => "Short edge (px)",
            SizeKind::Megapixels => "Megapixels",
            SizeKind::Percent => "Percent",
        }
    }
}

/// The form's fields. Each size kind keeps its own value, so switching back and forth does
/// not lose what was typed.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportForm {
    pub format: FormatKind,
    /// JPEG quality, 1..=100.
    pub quality: u8,
    /// WebP without loss (larger); off: lossy at `quality`.
    pub webp_lossless: bool,
    pub size: SizeKind,
    /// The long or short edge in pixels.
    pub pixels: u32,
    pub megapixels: f32,
    pub percent: f32,
    pub pattern: String,
    pub metadata: Metadata,
    pub overwrite: bool,
    pub folder: Option<PathBuf>,
    pub sharpen: Sharpen,
    /// The watermark is on; its fields below keep their values while it is off.
    pub watermark: bool,
    pub stamp: StampKind,
    pub stamp_text: String,
    pub logo: Option<PathBuf>,
    /// Percent of the photo's width.
    pub stamp_width: f32,
    /// Percent.
    pub stamp_opacity: f32,
    pub corner: Corner,
    /// The colour space written, and the last ICC profile picked (offered in the list).
    pub space: OutputSpace,
    pub profile: Option<PathBuf>,
}

impl Default for ExportForm {
    fn default() -> ExportForm {
        ExportForm {
            format: FormatKind::Jpeg,
            quality: DEFAULT_QUALITY,
            webp_lossless: true,
            size: SizeKind::Original,
            pixels: 2048,
            megapixels: 12.0,
            percent: 50.0,
            pattern: DEFAULT_PATTERN.to_owned(),
            metadata: Metadata::All,
            overwrite: false,
            folder: None,
            sharpen: Sharpen::Off,
            watermark: false,
            stamp: StampKind::Text,
            stamp_text: DEFAULT_STAMP.to_owned(),
            logo: None,
            stamp_width: 20.0,
            stamp_opacity: 60.0,
            corner: Corner::BottomRight,
            space: OutputSpace::Srgb,
            profile: None,
        }
    }
}

impl ExportForm {
    /// The form as `spec` left it.
    pub fn from_spec(spec: &ExportSpec) -> ExportForm {
        let mut f = ExportForm::default();
        match spec.format {
            Format::Jpeg { quality } => {
                f.format = FormatKind::Jpeg;
                f.quality = quality.clamp(1, 100);
            }
            Format::Png => f.format = FormatKind::Png,
            Format::WebP => f.format = FormatKind::WebP,
            Format::WebPLossy { quality } => {
                f.format = FormatKind::WebP;
                f.webp_lossless = false;
                f.quality = quality.clamp(1, 100);
            }
            Format::Avif { quality } => {
                f.format = FormatKind::Avif;
                f.quality = quality.clamp(1, 100);
            }
            Format::Tiff16 => f.format = FormatKind::Tiff16,
        }
        f.sharpen = spec.sharpen;
        if let Some(mark) = &spec.watermark {
            f.watermark = true;
            match &mark.stamp {
                Stamp::Text(text) => (f.stamp, f.stamp_text) = (StampKind::Text, text.clone()),
                Stamp::Image(path) => (f.stamp, f.logo) = (StampKind::Logo, Some(path.clone())),
            }
            f.stamp_width = (mark.width * 100.0).clamp(2.0, 100.0);
            f.stamp_opacity = (mark.opacity * 100.0).clamp(5.0, 100.0);
            f.corner = mark.corner;
        }
        match spec.size {
            Size::Original => f.size = SizeKind::Original,
            Size::LongEdge(n) => (f.size, f.pixels) = (SizeKind::LongEdge, n),
            Size::ShortEdge(n) => (f.size, f.pixels) = (SizeKind::ShortEdge, n),
            Size::Megapixels(m) => (f.size, f.megapixels) = (SizeKind::Megapixels, m),
            Size::Percent(p) => (f.size, f.percent) = (SizeKind::Percent, p),
        }
        spec.rename.0.clone_into(&mut f.pattern);
        f.metadata = spec.metadata;
        if let OutputSpace::Profile(p) = &spec.space {
            f.profile = Some(p.clone());
        }
        f.space = spec.space.clone();
        f.overwrite = spec.overwrite;
        let Destination::Folder(folder) = &spec.destination;
        f.folder = Some(folder.clone());
        f
    }

    /// The form remembered in `prefs.last_export` (an unreadable value gives the defaults).
    pub fn from_prefs(value: Option<&serde_json::Value>) -> ExportForm {
        let Some(value) = value else {
            return ExportForm::default();
        };
        match serde_json::from_value::<ExportSpec>(value.clone()) {
            Ok(spec) => ExportForm::from_spec(&spec),
            Err(e) => {
                log::warn!("the remembered export settings are unreadable ({e}); using defaults");
                ExportForm::default()
            }
        }
    }

    /// A note under the size field.
    pub fn size_hint(&self) -> Option<&'static str> {
        (self.size == SizeKind::Percent && self.percent > 100.0).then_some(ENLARGES)
    }

    /// The spec to export with, or the first problem (a sentence to show).
    pub fn spec(&self) -> Result<ExportSpec, &'static str> {
        let positive = |v: f32| v.is_finite() && v > 0.0;
        let size = match self.size {
            SizeKind::Original => Size::Original,
            SizeKind::LongEdge if self.pixels > 0 => Size::LongEdge(self.pixels),
            SizeKind::ShortEdge if self.pixels > 0 => Size::ShortEdge(self.pixels),
            SizeKind::Megapixels if positive(self.megapixels) => Size::Megapixels(self.megapixels),
            SizeKind::Percent if positive(self.percent) => Size::Percent(self.percent),
            _ => return Err(BAD_SIZE),
        };
        if self.pattern.trim().is_empty() {
            return Err(NO_PATTERN);
        }
        let Some(folder) = &self.folder else {
            return Err(NO_FOLDER);
        };
        let format = match self.format {
            FormatKind::Jpeg => Format::Jpeg {
                quality: self.quality.clamp(1, 100),
            },
            FormatKind::Png => Format::Png,
            FormatKind::WebP if self.webp_lossless => Format::WebP,
            FormatKind::WebP => Format::WebPLossy {
                quality: self.quality.clamp(1, 100),
            },
            FormatKind::Avif => Format::Avif {
                quality: self.quality.clamp(1, 100),
            },
            FormatKind::Tiff16 => Format::Tiff16,
        };
        let watermark = if self.watermark {
            let stamp = match self.stamp {
                StampKind::Text if self.stamp_text.trim().is_empty() => {
                    return Err(NO_WATERMARK_TEXT);
                }
                StampKind::Text => Stamp::Text(self.stamp_text.trim().to_owned()),
                StampKind::Logo => Stamp::Image(self.logo.clone().ok_or(NO_LOGO)?),
            };
            Some(Watermark {
                stamp,
                width: self.stamp_width.clamp(2.0, 100.0) / 100.0,
                opacity: self.stamp_opacity.clamp(5.0, 100.0) / 100.0,
                corner: self.corner,
            })
        } else {
            None
        };
        Ok(ExportSpec {
            format,
            size,
            destination: Destination::Folder(folder.clone()),
            rename: Pattern(self.pattern.trim().to_owned()),
            metadata: self.metadata,
            overwrite: self.overwrite,
            sharpen: self.sharpen,
            watermark,
            space: self.space.clone(),
        })
    }
}

/// Where the dialog is.
#[derive(Debug, Clone, PartialEq)]
pub enum Stage {
    Form,
    Running { done: usize, total: usize },
    Done(ExportOutcome),
}

/// What the dialog asks of the library this frame.
#[derive(Debug, Clone, PartialEq)]
pub enum ExportAction {
    None,
    /// Export with this spec (already validated).
    Start(ExportSpec),
    /// Open the system folder picker for this ticket.
    Pick(u64),
    /// Open the system file picker for the watermark's logo, for this ticket.
    PickLogo(u64),
    /// Open the system file picker for an ICC profile, for this ticket.
    PickProfile(u64),
    /// Check the typed path for this ticket.
    CheckTyped(u64, PathBuf),
    /// Open the folder holding this file.
    ShowFolder(PathBuf),
    /// The studio's export presets changed: remember these.
    SavePresets(Vec<super::export_presets::ExportPreset>),
    /// Hide the window (an export still running carries on).
    Close,
}

/// The open (or hidden, while running) export dialog.
pub struct ExportDialog {
    pub ids: Vec<i64>,
    pub form: ExportForm,
    pub stage: Stage,
    pub stop: Arc<AtomicBool>,
    /// Shown; false once closed while the export runs.
    pub visible: bool,
    /// The latest folder choice; older answers are ignored.
    ticket: u64,
    picking: bool,
    checking: bool,
    typed: String,
    /// A problem with the form, the folder or the last run, shown under the form.
    error: Option<String>,
    /// Export was pressed with a typed path still to check: start once it is confirmed.
    start_after_check: bool,
    /// The typed path was confirmed after Export: start on the next frame.
    start_ready: bool,
    /// The logo picker's latest ticket, and whether it is open.
    logo_ticket: u64,
    picking_logo: bool,
    profile_ticket: u64,
    picking_profile: bool,
    /// The studio's export presets (`prefs.export_presets`) and the bar choosing them.
    pub presets: Vec<super::export_presets::ExportPreset>,
    preset_bar: super::export_presets::PresetBar,
}

impl ExportDialog {
    pub fn new(ids: Vec<i64>, form: ExportForm) -> ExportDialog {
        ExportDialog {
            ids,
            form,
            stage: Stage::Form,
            stop: Arc::new(AtomicBool::new(false)),
            visible: true,
            ticket: 0,
            picking: false,
            checking: false,
            typed: String::new(),
            error: None,
            start_after_check: false,
            start_ready: false,
            logo_ticket: 0,
            picking_logo: false,
            profile_ticket: 0,
            picking_profile: false,
            presets: Vec::new(),
            preset_bar: super::export_presets::PresetBar::default(),
        }
    }

    /// The logo picker answered for `ticket`: the picture, `None` (cancelled) or a problem.
    /// The profile picker answered for `ticket`: an ICC file, `None` (cancelled) or a problem.
    pub fn profile_chosen(&mut self, ticket: u64, result: Result<Option<PathBuf>, String>) {
        if ticket != self.profile_ticket {
            return;
        }
        self.picking_profile = false;
        match result {
            Ok(Some(path)) => {
                match lf_catalog::colour::profile(&OutputSpace::Profile(path.clone())) {
                    Ok(_) => {
                        self.form.profile = Some(path.clone());
                        self.form.space = OutputSpace::Profile(path);
                    }
                    Err(e) => self.error = Some(e.to_string()),
                }
            }
            Ok(None) => {}
            Err(e) => self.error = Some(e),
        }
    }

    pub fn logo_chosen(&mut self, ticket: u64, result: Result<Option<PathBuf>, String>) {
        if ticket != self.logo_ticket {
            return;
        }
        self.picking_logo = false;
        match result {
            Ok(Some(path)) => {
                self.form.logo = Some(path);
                self.error = None;
            }
            Ok(None) => {}
            Err(e) => self.error = Some(e),
        }
    }

    pub fn running(&self) -> bool {
        matches!(self.stage, Stage::Running { .. })
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Replaces the typed folder path (tests use it).
    pub fn set_typed(&mut self, path: &str) {
        path.clone_into(&mut self.typed);
    }

    /// The job started.
    pub fn started(&mut self) {
        self.error = None;
        self.stop.store(false, Ordering::SeqCst);
        self.stage = Stage::Running {
            done: 0,
            total: self.ids.len(),
        };
    }

    pub fn progress(&mut self, done: usize, total: usize) {
        if self.running() {
            self.stage = Stage::Running { done, total };
        }
    }

    /// The job ended: the outcome, or why it was refused (back to the form, which keeps
    /// what was chosen).
    pub fn finished(&mut self, result: Result<ExportOutcome, String>) {
        match result {
            Ok(outcome) => self.stage = Stage::Done(outcome),
            Err(e) => {
                self.stage = Stage::Form;
                self.error = Some(e);
            }
        }
    }

    /// A folder choice answered for `ticket`: the folder, `None` (cancelled) or why it
    /// cannot be used.
    pub fn folder_chosen(&mut self, ticket: u64, result: Result<Option<PathBuf>, String>) {
        if ticket != self.ticket {
            return;
        }
        self.picking = false;
        self.checking = false;
        let start = std::mem::take(&mut self.start_after_check);
        match result {
            Ok(Some(path)) => {
                self.form.folder = Some(path);
                self.typed.clear();
                self.error = None;
                self.start_ready = start;
            }
            Ok(None) => {}
            Err(e) => self.error = Some(e),
        }
    }

    /// The Export button. A typed path not yet confirmed with "Use this folder" is checked
    /// first (off the UI thread) and the export starts once it is confirmed
    /// ([`ExportDialog::take_start`]); otherwise the form is validated and started.
    pub fn press_export(&mut self) -> ExportAction {
        let typed = self.typed.trim();
        if !typed.is_empty() {
            if self.checking {
                self.start_after_check = true;
                return ExportAction::None;
            }
            let path = PathBuf::from(typed);
            let ticket = self.next_ticket();
            self.checking = true;
            self.start_after_check = true;
            return ExportAction::CheckTyped(ticket, path);
        }
        match self.form.spec() {
            Ok(spec) => ExportAction::Start(spec),
            Err(e) => {
                self.error = Some(e.to_owned());
                ExportAction::None
            }
        }
    }

    /// An export to start now that a typed folder was confirmed (once).
    pub fn take_start(&mut self) -> Option<ExportSpec> {
        if !std::mem::take(&mut self.start_ready) {
            return None;
        }
        match self.form.spec() {
            Ok(spec) => Some(spec),
            Err(e) => {
                self.error = Some(e.to_owned());
                None
            }
        }
    }

    fn next_ticket(&mut self) -> u64 {
        self.ticket += 1;
        self.error = None;
        // A new choice (the picker) supersedes an export waiting on a typed path.
        self.start_after_check = false;
        self.ticket
    }

    pub fn show(&mut self, ctx: &egui::Context) -> ExportAction {
        if self.stage == Stage::Form
            && let Some(spec) = self.take_start()
        {
            return ExportAction::Start(spec);
        }
        let mut action = ExportAction::None;
        let _ = egui::Modal::new(egui::Id::unique("export-dialog")).show(ctx, |ui| {
            ui.set_max_width(460.0);
            ui.heading(title(self.ids.len()));
            ui.add_space(8.0);
            action = match self.stage.clone() {
                Stage::Form => self.form_ui(ui),
                Stage::Running { done, total } => self.running_ui(ui, done, total),
                Stage::Done(outcome) => done_ui(ui, &outcome),
            };
        });
        if action == ExportAction::None && ctx.input(|i| i.key_pressed(Key::Escape)) {
            action = ExportAction::Close;
        }
        action
    }

    fn form_ui(&mut self, ui: &mut egui::Ui) -> ExportAction {
        let mut action = ExportAction::None;
        if let Some(list) = self.preset_bar.ui(ui, &self.presets, &mut self.form) {
            self.presets.clone_from(&list);
            action = ExportAction::SavePresets(list);
        }
        ui.add_space(6.0);
        let f = &mut self.form;
        egui::Grid::new("export-form")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("Format");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut f.format, FormatKind::Jpeg, "JPEG");
                    ui.radio_value(&mut f.format, FormatKind::Png, "PNG");
                    ui.radio_value(&mut f.format, FormatKind::WebP, "WebP");
                    ui.radio_value(&mut f.format, FormatKind::Avif, "AVIF")
                        .on_hover_text("Smallest files at the same look; newer browsers and apps");
                    ui.radio_value(&mut f.format, FormatKind::Tiff16, "TIFF 16-bit");
                });
                ui.end_row();
                if f.format == FormatKind::WebP {
                    ui.label("");
                    ui.checkbox(&mut f.webp_lossless, "Lossless").on_hover_text(
                        "The same pixels, larger files; off is lossy at the quality below",
                    );
                    ui.end_row();
                }
                let lossy = match f.format {
                    FormatKind::Jpeg | FormatKind::Avif => true,
                    FormatKind::WebP => !f.webp_lossless,
                    _ => false,
                };
                if lossy {
                    let label = ui.label("Quality");
                    ui.add(egui::Slider::new(&mut f.quality, 1..=100))
                        .labelled_by(label.id);
                    ui.end_row();
                }
                ui.label("Size");
                ui.horizontal(|ui| {
                    let size = egui::ComboBox::from_id_salt("export-size")
                        .selected_text(f.size.label())
                        .show_ui(ui, |ui| {
                            for kind in SizeKind::ALL {
                                ui.selectable_value(&mut f.size, kind, kind.label());
                            }
                        });
                    size.response.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::ComboBox, true, "Size")
                    });
                    match f.size {
                        SizeKind::Original => {}
                        SizeKind::LongEdge | SizeKind::ShortEdge => {
                            ui.add(egui::DragValue::new(&mut f.pixels).range(0..=100_000));
                        }
                        SizeKind::Megapixels => {
                            ui.add(
                                egui::DragValue::new(&mut f.megapixels)
                                    .range(0.0..=1_000.0)
                                    .speed(0.1),
                            );
                        }
                        SizeKind::Percent => {
                            ui.add(
                                egui::DragValue::new(&mut f.percent)
                                    .range(0.0..=1_000.0)
                                    .suffix(" %"),
                            );
                        }
                    }
                });
                ui.end_row();
                if let Some(hint) = f.size_hint() {
                    ui.label("");
                    ui.label(RichText::new(hint).color(theme::MUTED).small());
                    ui.end_row();
                }
                ui.label(COLOUR_SPACE);
                ui.horizontal(|ui| {
                    let combo = egui::ComboBox::from_id_salt("export-space")
                        .selected_text(f.space.name())
                        .width(200.0)
                        .show_ui(ui, |ui| {
                            for s in [
                                OutputSpace::Srgb,
                                OutputSpace::DisplayP3,
                                OutputSpace::AdobeRgb,
                            ] {
                                let name = s.name();
                                ui.selectable_value(&mut f.space, s, name);
                            }
                            if let Some(p) = &f.profile {
                                let s = OutputSpace::Profile(p.clone());
                                let name = s.name();
                                ui.selectable_value(&mut f.space, s, name);
                            }
                        });
                    combo.response.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::ComboBox, true, COLOUR_SPACE)
                    });
                    if ui
                        .add_enabled(!self.picking_profile, egui::Button::new(CHOOSE_PROFILE))
                        .on_hover_text("A lab's printer and paper profile (.icc or .icm, RGB)")
                        .clicked()
                    {
                        self.picking_profile = true;
                        action = ExportAction::PickProfile(0);
                    }
                });
                ui.end_row();
                let label = ui.label("File name");
                ui.vertical(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut f.pattern).desired_width(260.0))
                        .labelled_by(label.id);
                    ui.label(RichText::new(TOKENS).color(theme::MUTED).small());
                });
                ui.end_row();
                ui.label("Metadata");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut f.metadata, Metadata::All, "All");
                    ui.radio_value(&mut f.metadata, Metadata::CopyrightOnly, "Copyright only");
                    ui.radio_value(&mut f.metadata, Metadata::None, "None");
                });
                ui.end_row();
                ui.label(SHARPEN);
                ui.horizontal(|ui| {
                    for s in Sharpen::ALL {
                        ui.radio_value(&mut f.sharpen, s, s.label());
                    }
                });
                ui.end_row();
                ui.label("Watermark");
                ui.checkbox(&mut f.watermark, WATERMARK);
                ui.end_row();
                if f.watermark {
                    ui.label("");
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut f.stamp, StampKind::Text, "Text");
                            ui.radio_value(&mut f.stamp, StampKind::Logo, "Logo picture");
                        });
                        match f.stamp {
                            StampKind::Text => {
                                let label = ui.label(WATERMARK_TEXT);
                                ui.add(
                                    egui::TextEdit::singleline(&mut f.stamp_text)
                                        .desired_width(260.0),
                                )
                                .labelled_by(label.id);
                            }
                            StampKind::Logo => {
                                match &f.logo {
                                    Some(logo) => ui.label(logo.display().to_string()),
                                    None => ui.label(
                                        RichText::new("No picture chosen.").color(theme::MUTED),
                                    ),
                                };
                                if ui
                                    .add_enabled(!self.picking_logo, egui::Button::new(CHOOSE_LOGO))
                                    .on_hover_text("A PNG with a transparent background works best")
                                    .clicked()
                                {
                                    self.picking_logo = true;
                                    action = ExportAction::PickLogo(0);
                                }
                            }
                        }
                        ui.add(
                            egui::Slider::new(&mut f.stamp_width, 2.0..=100.0)
                                .text("width (% of the photo)")
                                .fixed_decimals(0),
                        );
                        ui.add(
                            egui::Slider::new(&mut f.stamp_opacity, 5.0..=100.0)
                                .text("opacity (%)")
                                .fixed_decimals(0),
                        );
                        let corner = egui::ComboBox::from_id_salt("watermark-corner")
                            .selected_text(f.corner.label())
                            .show_ui(ui, |ui| {
                                for c in Corner::ALL {
                                    ui.selectable_value(&mut f.corner, c, c.label());
                                }
                            });
                        corner.response.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::Role::ComboBox,
                                true,
                                "Watermark position",
                            )
                        });
                    });
                    ui.end_row();
                }
                ui.label("");
                ui.checkbox(&mut f.overwrite, REPLACE);
                ui.end_row();
                ui.label("Destination folder");
                ui.vertical(|ui| {
                    match &f.folder {
                        Some(folder) => ui.label(folder.display().to_string()),
                        None => ui.label(RichText::new("No folder chosen.").color(theme::MUTED)),
                    };
                    let text = if self.picking { WAITING } else { CHOOSE };
                    if ui
                        .add_enabled(!self.picking, egui::Button::new(text))
                        .clicked()
                    {
                        self.picking = true;
                        self.checking = false;
                        action = ExportAction::Pick(0);
                    }
                    let label = ui.label(TYPE_PATH);
                    let field = ui
                        .add(egui::TextEdit::singleline(&mut self.typed).desired_width(260.0))
                        .labelled_by(label.id);
                    let entered = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    let can_use = !self.checking && !self.typed.trim().is_empty();
                    let clicked = ui
                        .add_enabled(can_use, egui::Button::new(USE_FOLDER))
                        .clicked();
                    if clicked || (entered && can_use) {
                        self.checking = true;
                        action = ExportAction::CheckTyped(0, PathBuf::from(self.typed.trim()));
                    }
                });
                ui.end_row();
            });
        // The tickets are taken once the form is no longer borrowed.
        action = match action {
            ExportAction::Pick(_) => ExportAction::Pick(self.next_ticket()),
            ExportAction::PickLogo(_) => {
                self.logo_ticket += 1;
                ExportAction::PickLogo(self.logo_ticket)
            }
            ExportAction::PickProfile(_) => {
                self.profile_ticket += 1;
                ExportAction::PickProfile(self.profile_ticket)
            }
            ExportAction::CheckTyped(_, path) => ExportAction::CheckTyped(self.next_ticket(), path),
            other => other,
        };
        if let Some(error) = &self.error {
            ui.add_space(6.0);
            ui.label(RichText::new(error).color(theme::DANGER));
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui.button(EXPORT).clicked() {
                action = self.press_export();
            }
            if ui.button(CANCEL).clicked() {
                action = ExportAction::Close;
            }
        });
        action
    }

    fn running_ui(&mut self, ui: &mut egui::Ui, done: usize, total: usize) -> ExportAction {
        let mut action = ExportAction::None;
        ui.add(egui::ProgressBar::new(done as f32 / total.max(1) as f32));
        ui.add_space(6.0);
        ui.label(progress_text(done, total));
        ui.label(RichText::new(KEEPS_RUNNING).color(theme::MUTED).small());
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let stopping = self.stop.load(Ordering::SeqCst);
            if ui.add_enabled(!stopping, egui::Button::new(STOP)).clicked() {
                self.stop.store(true, Ordering::SeqCst);
            }
            if ui.button(CLOSE).clicked() {
                action = ExportAction::Close;
            }
        });
        action
    }
}

/// The closing line, the failed photos (file name and the error) and the buttons.
fn done_ui(ui: &mut egui::Ui, outcome: &ExportOutcome) -> ExportAction {
    let mut action = ExportAction::None;
    ui.label(crate::export_job::done_text(outcome));
    if !outcome.failed.is_empty() {
        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .max_height(220.0)
            .show(ui, |ui| {
                for Failed { file, error, .. } in &outcome.failed {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(file).strong());
                        ui.label(RichText::new(error).color(theme::DANGER));
                    });
                }
            });
    }
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        if let Some(first) = outcome.exported.first()
            && ui.button(SHOW_FOLDER).clicked()
        {
            action = ExportAction::ShowFolder(first.path.clone());
        }
        if ui.button(DONE).clicked() {
            action = ExportAction::Close;
        }
    });
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_from_form_defaults() {
        let mut form = ExportForm::default();
        assert_eq!(form.spec(), Err(NO_FOLDER), "a folder must be chosen");
        form.folder = Some(PathBuf::from("out"));
        assert_eq!(
            form.spec(),
            Ok(ExportSpec {
                format: Format::Jpeg { quality: 93 },
                size: Size::Original,
                destination: Destination::Folder(PathBuf::from("out")),
                rename: Pattern("{name}".into()),
                metadata: Metadata::All,
                overwrite: false,
                sharpen: Sharpen::Off,
                watermark: None,
                space: Default::default(),
            })
        );
        assert_eq!(ExportForm::from_prefs(None), ExportForm::default());
        let junk = serde_json::json!({ "format": "jpeg", "quality": 90 });
        assert_eq!(ExportForm::from_prefs(Some(&junk)), ExportForm::default());

        // What prefs remember comes back as it was.
        for spec in [
            ExportSpec {
                format: Format::Tiff16,
                size: Size::LongEdge(3000),
                destination: Destination::Folder(PathBuf::from("a")),
                rename: Pattern("{date}-{seq}".into()),
                metadata: Metadata::CopyrightOnly,
                overwrite: true,
                sharpen: Sharpen::Off,
                watermark: None,
                space: Default::default(),
            },
            ExportSpec {
                format: Format::Jpeg { quality: 70 },
                size: Size::Megapixels(8.5),
                destination: Destination::Folder(PathBuf::from("b")),
                rename: Pattern("{camera}_{name}".into()),
                metadata: Metadata::None,
                overwrite: false,
                sharpen: Sharpen::Off,
                watermark: None,
                space: Default::default(),
            },
            ExportSpec {
                format: Format::Png,
                size: Size::Percent(25.0),
                destination: Destination::Folder(PathBuf::from("c")),
                rename: Pattern("{name}".into()),
                metadata: Metadata::All,
                overwrite: false,
                sharpen: Sharpen::Off,
                watermark: None,
                space: Default::default(),
            },
            ExportSpec {
                format: Format::Png,
                size: Size::ShortEdge(1080),
                destination: Destination::Folder(PathBuf::from("d")),
                rename: Pattern("{name}".into()),
                metadata: Metadata::All,
                overwrite: false,
                sharpen: Sharpen::Off,
                watermark: None,
                space: Default::default(),
            },
        ] {
            let value = serde_json::to_value(&spec).unwrap();
            let form = ExportForm::from_prefs(Some(&value));
            assert_eq!(form.spec(), Ok(spec));
        }
        // A remembered PNG keeps the default quality for when JPEG is chosen again.
        let png = ExportForm::from_spec(&ExportSpec {
            format: Format::Png,
            size: Size::Original,
            destination: Destination::Folder(PathBuf::from("c")),
            rename: Pattern("{name}".into()),
            metadata: Metadata::All,
            overwrite: false,
            sharpen: Sharpen::Off,
            watermark: None,
            space: Default::default(),
        });
        assert_eq!(png.quality, DEFAULT_QUALITY);
    }

    #[test]
    fn sharpening_and_the_watermark_round_trip_and_are_checked() {
        let mut form = ExportForm {
            folder: Some(PathBuf::from("out")),
            format: FormatKind::WebP,
            sharpen: Sharpen::High,
            watermark: true,
            stamp_text: "  © Sharma Studio ".into(),
            stamp_width: 25.0,
            stamp_opacity: 70.0,
            corner: Corner::TopLeft,
            ..ExportForm::default()
        };
        let spec = form.spec().unwrap();
        assert_eq!(spec.format, Format::WebP);
        assert_eq!(spec.sharpen, Sharpen::High);
        assert_eq!(
            spec.watermark,
            Some(Watermark {
                stamp: Stamp::Text("© Sharma Studio".into()),
                width: 0.25,
                opacity: 0.7,
                corner: Corner::TopLeft,
            })
        );
        // Remembered in prefs and read back as it was.
        let back = ExportForm::from_prefs(Some(&serde_json::to_value(&spec).unwrap()));
        assert_eq!(back.spec(), Ok(spec));
        // A spec saved before these existed reads as no sharpening and no watermark.
        let mut old = serde_json::to_value(form.spec().unwrap()).unwrap();
        old.as_object_mut().unwrap().remove("sharpen");
        old.as_object_mut().unwrap().remove("watermark");
        let old = ExportForm::from_prefs(Some(&old));
        assert_eq!((old.sharpen, old.watermark), (Sharpen::Off, false));

        // An empty text, or a logo not chosen, is refused with a sentence to show.
        form.stamp_text = "  ".into();
        assert_eq!(form.spec(), Err(NO_WATERMARK_TEXT));
        form.stamp = StampKind::Logo;
        assert_eq!(form.spec(), Err(NO_LOGO));
        form.logo = Some(PathBuf::from("logo.png"));
        assert!(matches!(
            form.spec().unwrap().watermark.unwrap().stamp,
            Stamp::Image(_)
        ));
        // Off: nothing is checked and nothing is stamped.
        form.watermark = false;
        form.logo = None;
        assert_eq!(form.spec().unwrap().watermark, None);
    }

    #[test]
    fn invalid_size_blocked() {
        let ok = ExportForm {
            folder: Some(PathBuf::from("out")),
            ..ExportForm::default()
        };
        for (size, pixels, megapixels, percent) in [
            (SizeKind::LongEdge, 0, 12.0, 50.0),
            (SizeKind::ShortEdge, 0, 12.0, 50.0),
            (SizeKind::Megapixels, 2048, 0.0, 50.0),
            (SizeKind::Megapixels, 2048, -1.0, 50.0),
            (SizeKind::Megapixels, 2048, f32::NAN, 50.0),
            (SizeKind::Percent, 2048, 12.0, 0.0),
            (SizeKind::Percent, 2048, 12.0, f32::INFINITY),
        ] {
            let form = ExportForm {
                size,
                pixels,
                megapixels,
                percent,
                ..ok.clone()
            };
            assert_eq!(
                form.spec(),
                Err(BAD_SIZE),
                "{size:?} {pixels} {megapixels} {percent}"
            );
        }
        // Only the chosen kind's value counts.
        let original = ExportForm {
            pixels: 0,
            percent: 0.0,
            ..ok.clone()
        };
        assert!(original.spec().is_ok());
        let blank = ExportForm {
            pattern: "   ".into(),
            ..ok.clone()
        };
        assert_eq!(blank.spec(), Err(NO_PATTERN));
        let quality = ExportForm { quality: 0, ..ok };
        assert_eq!(
            quality.spec().unwrap().format,
            Format::Jpeg { quality: 1 },
            "quality is kept within 1..=100"
        );
    }

    #[test]
    fn percent_above_100_says_it_enlarges() {
        let mut f = ExportForm {
            size: SizeKind::Percent,
            percent: 150.0,
            ..ExportForm::default()
        };
        assert_eq!(f.size_hint(), Some(ENLARGES));
        f.percent = 100.0;
        assert_eq!(f.size_hint(), None);
        f.percent = 150.0;
        f.size = SizeKind::Original;
        assert_eq!(f.size_hint(), None, "only for Percent");
    }

    #[test]
    fn a_typed_path_not_yet_used_is_checked_then_exported_to() {
        let mut d = ExportDialog::new(vec![1], ExportForm::default());
        d.set_typed("typed");
        // Export with a typed path pending: it is checked first, not ignored.
        let ticket = match d.press_export() {
            ExportAction::CheckTyped(ticket, path) => {
                assert_eq!(path, PathBuf::from("typed"));
                ticket
            }
            other => panic!("expected a check, got {other:?}"),
        };
        assert_eq!(d.take_start(), None, "not before the answer");
        d.folder_chosen(ticket, Ok(Some(PathBuf::from("typed"))));
        match d.take_start() {
            Some(spec) => assert_eq!(
                spec.destination,
                Destination::Folder(PathBuf::from("typed"))
            ),
            None => panic!("the export starts once the folder is confirmed"),
        }
        assert_eq!(d.take_start(), None, "once");

        // A refused typed path does not start anything and says why.
        let mut d = ExportDialog::new(vec![1], ExportForm::default());
        d.set_typed("bad");
        let ExportAction::CheckTyped(ticket, _) = d.press_export() else {
            panic!("expected a check");
        };
        d.folder_chosen(ticket, Err("That folder does not exist.".into()));
        assert_eq!(d.take_start(), None);
        assert_eq!(d.error(), Some("That folder does not exist."));

        // No typed path: the form's own folder, as before.
        let mut d = ExportDialog::new(
            vec![1],
            ExportForm {
                folder: Some(PathBuf::from("out")),
                ..ExportForm::default()
            },
        );
        assert!(matches!(d.press_export(), ExportAction::Start(_)));
        let mut d = ExportDialog::new(vec![1], ExportForm::default());
        assert_eq!(d.press_export(), ExportAction::None);
        assert_eq!(d.error(), Some(NO_FOLDER));
    }

    #[test]
    fn a_refused_run_returns_to_the_form_with_its_message() {
        let mut d = ExportDialog::new(vec![1, 2], ExportForm::default());
        d.started();
        assert!(d.running());
        d.progress(1, 2);
        assert_eq!(d.stage, Stage::Running { done: 1, total: 2 });
        d.finished(Err("IMG_1.jpg already exists.".into()));
        assert_eq!(d.stage, Stage::Form);
        assert_eq!(d.error(), Some("IMG_1.jpg already exists."));
        // A stale folder answer is ignored; the current one sets the folder.
        d.ticket = 3;
        d.folder_chosen(2, Ok(Some(PathBuf::from("old"))));
        assert_eq!(d.form.folder, None);
        d.folder_chosen(3, Ok(Some(PathBuf::from("new"))));
        assert_eq!(d.form.folder, Some(PathBuf::from("new")));
        assert_eq!(title(1), "Export 1 photo");
        assert_eq!(title(1_200), "Export 1,200 photos");
        assert_eq!(status_text(12, 40), "Exporting 12 of 40");
    }
}
