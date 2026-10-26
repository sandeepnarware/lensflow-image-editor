//! "Export N photos" (`Ctrl+Shift+E`): format, size, file name pattern, metadata, replace,
//! destination folder. The form is remembered in `prefs.last_export` as the `ExportSpec`
//! JSON. The work is [`crate::export_job::run`] on the `Io` lane; while it runs the dialog
//! shows "12 of 40" and Stop, and closing the window leaves the export running (the status
//! bar keeps the count and a toast tells how it ended).

use crate::catalog_ext::count;
use crate::export_job::{ExportOutcome, Failed};
use crate::theme;
use eframe::egui::{self, Key, RichText};
use lf_catalog::export::{Destination, ExportSpec, Format, Metadata, Pattern, Size};
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
pub const TOKENS: &str = "Tokens: {name} {seq} {date} {camera}";
pub const KEEPS_RUNNING: &str = "Closing this window does not stop the export.";
/// Validation: shown under the Export button.
pub const NO_FOLDER: &str = "Choose a folder to export to.";
pub const NO_PATTERN: &str = "Enter a file name pattern.";
pub const BAD_SIZE: &str = "Enter a size larger than 0.";
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
    Tiff16,
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
    pub size: SizeKind,
    /// The long or short edge in pixels.
    pub pixels: u32,
    pub megapixels: f32,
    pub percent: f32,
    pub pattern: String,
    pub metadata: Metadata,
    pub overwrite: bool,
    pub folder: Option<PathBuf>,
}

impl Default for ExportForm {
    fn default() -> ExportForm {
        ExportForm {
            format: FormatKind::Jpeg,
            quality: DEFAULT_QUALITY,
            size: SizeKind::Original,
            pixels: 2048,
            megapixels: 12.0,
            percent: 50.0,
            pattern: DEFAULT_PATTERN.to_owned(),
            metadata: Metadata::All,
            overwrite: false,
            folder: None,
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
            Format::Tiff16 => f.format = FormatKind::Tiff16,
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
            FormatKind::Tiff16 => Format::Tiff16,
        };
        Ok(ExportSpec {
            format,
            size,
            destination: Destination::Folder(folder.clone()),
            rename: Pattern(self.pattern.trim().to_owned()),
            metadata: self.metadata,
            overwrite: self.overwrite,
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
    /// Check the typed path for this ticket.
    CheckTyped(u64, PathBuf),
    /// Open the folder holding this file.
    ShowFolder(PathBuf),
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
        match result {
            Ok(Some(path)) => {
                self.form.folder = Some(path);
                self.typed.clear();
                self.error = None;
            }
            Ok(None) => {}
            Err(e) => self.error = Some(e),
        }
    }

    fn next_ticket(&mut self) -> u64 {
        self.ticket += 1;
        self.error = None;
        self.ticket
    }

    pub fn show(&mut self, ctx: &egui::Context) -> ExportAction {
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
        let f = &mut self.form;
        egui::Grid::new("export-form")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("Format");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut f.format, FormatKind::Jpeg, "JPEG");
                    ui.radio_value(&mut f.format, FormatKind::Png, "PNG");
                    ui.radio_value(&mut f.format, FormatKind::Tiff16, "TIFF 16-bit");
                });
                ui.end_row();
                if f.format == FormatKind::Jpeg {
                    ui.label("Quality");
                    ui.add(egui::Slider::new(&mut f.quality, 1..=100));
                    ui.end_row();
                }
                ui.label("Size");
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("export-size")
                        .selected_text(f.size.label())
                        .show_ui(ui, |ui| {
                            for kind in SizeKind::ALL {
                                ui.selectable_value(&mut f.size, kind, kind.label());
                            }
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
                match self.form.spec() {
                    Ok(spec) => action = ExportAction::Start(spec),
                    Err(e) => self.error = Some(e.to_owned()),
                }
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
            },
            ExportSpec {
                format: Format::Jpeg { quality: 70 },
                size: Size::Megapixels(8.5),
                destination: Destination::Folder(PathBuf::from("b")),
                rename: Pattern("{camera}_{name}".into()),
                metadata: Metadata::None,
                overwrite: false,
            },
            ExportSpec {
                format: Format::Png,
                size: Size::Percent(25.0),
                destination: Destination::Folder(PathBuf::from("c")),
                rename: Pattern("{name}".into()),
                metadata: Metadata::All,
                overwrite: false,
            },
            ExportSpec {
                format: Format::Png,
                size: Size::ShortEdge(1080),
                destination: Destination::Folder(PathBuf::from("d")),
                rename: Pattern("{name}".into()),
                metadata: Metadata::All,
                overwrite: false,
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
        });
        assert_eq!(png.quality, DEFAULT_QUALITY);
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
