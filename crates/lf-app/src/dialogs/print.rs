//! File → Print (`Ctrl+P`): the paper, which way up, how many photos a page (one, or a grid
//! for a contact sheet), the margins and a caption; then "Print…" writes the PDF to a
//! temporary file and opens it in the system's viewer to print, and "Save PDF…" asks where
//! to keep it (for a lab). The work is [`crate::print_job::run`] on the `Long` lane.

use crate::catalog_ext::count;
use crate::print_job::PrintRun;
use crate::theme;
use eframe::egui::{self, Key, RichText};
use lf_catalog::print::{Caption, Orientation, PAPERS, PrintSpec};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub const PAPER: &str = "Paper";
pub const LAYOUT: &str = "Photos per page";
pub const ORIENTATION: &str = "Orientation";
pub const CAPTION: &str = "Caption";
pub const MARGIN: &str = "Margins (mm)";
pub const PRINT: &str = "Print…";
pub const SAVE: &str = "Save PDF…";
pub const CANCEL: &str = "Cancel";
pub const CLOSE: &str = "Close";
pub const STOP: &str = "Stop";

/// The layouts offered: across, down, name.
pub const LAYOUTS: [(u32, u32, &str); 6] = [
    (1, 1, "1 per page"),
    (1, 2, "2 per page"),
    (2, 2, "4 per page"),
    (3, 3, "9 per page (contact sheet)"),
    (4, 5, "20 per page (contact sheet)"),
    (6, 8, "48 per page (contact sheet)"),
];
pub const SHEET_TITLE: &str = "Sheet title";

/// "Print 3 photos".
pub fn title(n: usize) -> String {
    if n == 1 {
        "Print 1 photo".to_owned()
    } else {
        format!("Print {} photos", count(n))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stage {
    Form,
    Running {
        done: usize,
        total: usize,
    },
    /// The PDF was written here (and opened, when printing).
    Done(PathBuf, bool),
}

/// What the dialog asks of the library this frame.
#[derive(Debug, Clone, PartialEq)]
pub enum PrintAction {
    None,
    /// Write the PDF to a temporary file and open it.
    Print(PrintSpec),
    /// Ask where to save it, then write it there.
    Save(PrintSpec),
    Close,
}

pub struct PrintDialog {
    pub ids: Vec<i64>,
    pub stage: Stage,
    pub stop: Arc<AtomicBool>,
    pub paper: usize,
    pub layout: usize,
    pub orientation: Orientation,
    pub caption: Caption,
    pub margin_mm: f64,
    /// A contact sheet's title, printed at the top of each page.
    pub title: String,
    error: Option<String>,
}

impl PrintDialog {
    pub fn new(ids: Vec<i64>) -> PrintDialog {
        PrintDialog {
            ids,
            stage: Stage::Form,
            stop: Arc::new(AtomicBool::new(false)),
            paper: 0,
            layout: 0,
            orientation: Orientation::Auto,
            caption: Caption::None,
            margin_mm: 10.0,
            title: String::new(),
            error: None,
        }
    }

    pub fn running(&self) -> bool {
        matches!(self.stage, Stage::Running { .. })
    }

    /// The layout the form describes.
    pub fn spec(&self) -> PrintSpec {
        let (cols, rows, _) = LAYOUTS[self.layout.min(LAYOUTS.len() - 1)];
        PrintSpec {
            paper: PAPERS[self.paper.min(PAPERS.len() - 1)],
            orientation: self.orientation,
            cols,
            rows,
            margin_mm: self.margin_mm.clamp(0.0, 50.0),
            // A contact sheet keeps a little room between photos; one a page needs none.
            gap_mm: if cols * rows > 1 { 5.0 } else { 0.0 },
            caption: self.caption,
        }
    }

    /// The run is starting for `job`.
    pub fn started(&mut self, _job: &PrintRun) {
        self.stop.store(false, Ordering::SeqCst);
        self.error = None;
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

    pub fn finished(&mut self, result: Result<(PathBuf, bool), String>) {
        match result {
            Ok((path, opened)) => self.stage = Stage::Done(path, opened),
            Err(e) => {
                self.error = Some(e);
                self.stage = Stage::Form;
            }
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> PrintAction {
        let mut action = PrintAction::None;
        let _ = egui::Modal::new(egui::Id::unique("print-dialog")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.heading(title(self.ids.len()));
            ui.add_space(8.0);
            action = match self.stage.clone() {
                Stage::Form => self.form_ui(ui),
                Stage::Running { done, total } => self.running_ui(ui, done, total),
                Stage::Done(path, opened) => done_ui(ui, &path, opened),
            };
        });
        action
    }

    fn form_ui(&mut self, ui: &mut egui::Ui) -> PrintAction {
        let mut action = PrintAction::None;
        egui::Grid::new("print-form")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label(PAPER);
                let r = egui::ComboBox::from_id_salt("print-paper")
                    .selected_text(PAPERS[self.paper].name)
                    .show_ui(ui, |ui| {
                        for (i, p) in PAPERS.iter().enumerate() {
                            ui.selectable_value(&mut self.paper, i, p.name);
                        }
                    });
                named(&r.response, PAPER);
                ui.end_row();

                ui.label(LAYOUT);
                let r = egui::ComboBox::from_id_salt("print-layout")
                    .selected_text(LAYOUTS[self.layout].2)
                    .width(200.0)
                    .show_ui(ui, |ui| {
                        for (i, l) in LAYOUTS.iter().enumerate() {
                            ui.selectable_value(&mut self.layout, i, l.2);
                        }
                    });
                named(&r.response, LAYOUT);
                ui.end_row();

                ui.label(ORIENTATION);
                ui.horizontal(|ui| {
                    for (o, name) in [
                        (Orientation::Auto, "Automatic"),
                        (Orientation::Portrait, "Portrait"),
                        (Orientation::Landscape, "Landscape"),
                    ] {
                        ui.radio_value(&mut self.orientation, o, name);
                    }
                });
                ui.end_row();

                ui.label(CAPTION);
                ui.horizontal(|ui| {
                    for (c, name) in [
                        (Caption::None, "None"),
                        (Caption::FileName, "File name"),
                        (Caption::Title, "Title"),
                        (Caption::Details, "Details"),
                    ] {
                        ui.radio_value(&mut self.caption, c, name);
                    }
                });
                ui.end_row();

                ui.label(MARGIN);
                ui.add(egui::Slider::new(&mut self.margin_mm, 0.0..=30.0).step_by(1.0));
                ui.end_row();

                if self.spec().per_page() > 1 {
                    let label = ui.label(SHEET_TITLE);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.title)
                            .hint_text("Printed at the top of each page"),
                    )
                    .labelled_by(label.id);
                    ui.end_row();
                }
            });
        let spec = self.spec();
        let pages = self.ids.len().div_ceil(spec.per_page());
        ui.label(
            RichText::new(format!(
                "{} {}, photos rendered at 300 dpi.",
                count(pages),
                if pages == 1 { "page" } else { "pages" }
            ))
            .color(theme::MUTED),
        );
        if let Some(e) = &self.error {
            ui.label(RichText::new(e).color(theme::WARNING));
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui.button(PRINT).clicked() {
                action = PrintAction::Print(spec);
            }
            if ui.button(SAVE).clicked() {
                action = PrintAction::Save(spec);
            }
            if ui.button(CANCEL).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                action = PrintAction::Close;
            }
        });
        action
    }

    fn running_ui(&mut self, ui: &mut egui::Ui, done: usize, total: usize) -> PrintAction {
        let fraction = if total == 0 {
            0.0
        } else {
            done as f32 / total as f32
        };
        ui.add(egui::ProgressBar::new(fraction).text(format!(
            "Preparing {} of {}",
            count(done),
            count(total)
        )));
        ui.add_space(8.0);
        let stopping = self.stop.load(Ordering::SeqCst);
        if ui.add_enabled(!stopping, egui::Button::new(STOP)).clicked() {
            self.stop.store(true, Ordering::SeqCst);
        }
        PrintAction::None
    }
}

fn done_ui(ui: &mut egui::Ui, path: &std::path::Path, opened: bool) -> PrintAction {
    if opened {
        ui.label("The PDF is open in your PDF viewer: print it from there.");
    } else {
        ui.label(format!("Saved {}.", path.display()));
    }
    ui.add_space(12.0);
    if ui.button(CLOSE).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
        PrintAction::Close
    } else {
        PrintAction::None
    }
}

fn named(response: &egui::Response, name: &str) {
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, name));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_form_makes_the_spec() {
        let mut d = PrintDialog::new(vec![1, 2, 3, 4, 5]);
        let one = d.spec();
        assert_eq!((one.cols, one.rows, one.gap_mm), (1, 1, 0.0));
        d.layout = 3;
        d.paper = 1;
        d.caption = Caption::Title;
        let sheet = d.spec();
        assert_eq!((sheet.cols, sheet.rows, sheet.gap_mm), (3, 3, 5.0));
        assert_eq!(sheet.paper.name, "Letter");
        assert_eq!(sheet.caption, Caption::Title);
        assert_eq!(title(5), "Print 5 photos");
    }

    #[test]
    fn a_failed_run_goes_back_to_the_form_with_why() {
        let mut d = PrintDialog::new(vec![1]);
        let job = PrintRun {
            spec: d.spec(),
            out: "x.pdf".into(),
            open: true,
            title: String::new(),
        };
        d.started(&job);
        assert!(d.running());
        d.finished(Err("disk full".into()));
        assert_eq!(d.stage, Stage::Form);
        assert_eq!(d.error.as_deref(), Some("disk full"));
    }
}
