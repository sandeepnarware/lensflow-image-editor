//! "Export to gallery": the studio's galleries (searchable), a section, the size and JPEG
//! quality, then the upload's progress (with Stop; closing the window leaves it running)
//! and how it ended. The work is [`crate::gallery_job::run`] on the `Long` lane; the lists
//! come from the `Account` lane. Shown only while the gate allows gallery export.

use crate::catalog_ext::count;
use crate::gallery_job::{GalleryOutcome, GalleryRun, done_text};
use crate::theme;
use eframe::egui::{self, Key, RichText};
use lf_account::galleries::{Gallery, Section};
use lf_catalog::export::Size;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub const SEARCH: &str = "Search galleries";
pub const NO_SECTION: &str = "No section";
pub const NO_GALLERIES: &str = "No galleries found. Make one on LensFlow first.";
pub const LOADING: &str = "Loading galleries…";
pub const CHOOSE: &str = "Choose a gallery.";
pub const UPLOAD: &str = "Upload";
pub const STOP: &str = "Stop";
pub const CLOSE: &str = "Close";
pub const DONE: &str = "Done";
pub const CANCEL: &str = "Cancel";
pub const KEEPS_RUNNING: &str = "Closing this window does not stop the upload.";
/// The default long edge and quality: big enough for a gallery, light enough to send.
pub const DEFAULT_EDGE: u32 = 3000;
pub const DEFAULT_QUALITY: u8 = 90;

/// "Export 3 photos to a gallery".
pub fn title(n: usize) -> String {
    if n == 1 {
        "Export 1 photo to a gallery".to_owned()
    } else {
        format!("Export {} photos to a gallery", count(n))
    }
}

/// Where the dialog is.
#[derive(Debug, Clone, PartialEq)]
pub enum Stage {
    Form,
    Running { done: usize, total: usize },
    Done(GalleryOutcome),
}

/// What the dialog asks of the library this frame.
#[derive(Debug, Clone, PartialEq)]
pub enum GalleryAction {
    None,
    /// List the galleries matching this search, for this ticket.
    Search(u64, String),
    /// List this gallery's sections.
    Sections(i64),
    Start(GalleryRun),
    /// Hide the window (an upload still running carries on).
    Close,
}

pub struct GalleryDialog {
    pub ids: Vec<i64>,
    pub stage: Stage,
    pub stop: Arc<AtomicBool>,
    pub visible: bool,
    /// None while the list is on its way.
    galleries: Option<Result<Vec<Gallery>, String>>,
    ticket: u64,
    search: String,
    pub chosen: Option<i64>,
    /// The chosen gallery's sections (None while on their way).
    sections: Option<Vec<Section>>,
    pub section: Option<i64>,
    pub long_edge: bool,
    pub pixels: u32,
    pub quality: u8,
    error: Option<String>,
}

impl GalleryDialog {
    /// A new dialog for `ids`; the first action it asks for is the gallery list.
    pub fn new(ids: Vec<i64>) -> (GalleryDialog, GalleryAction) {
        let dialog = GalleryDialog {
            ids,
            stage: Stage::Form,
            stop: Arc::new(AtomicBool::new(false)),
            visible: true,
            galleries: None,
            ticket: 1,
            search: String::new(),
            chosen: None,
            sections: None,
            section: None,
            long_edge: true,
            pixels: DEFAULT_EDGE,
            quality: DEFAULT_QUALITY,
            error: None,
        };
        (dialog, GalleryAction::Search(1, String::new()))
    }

    pub fn running(&self) -> bool {
        matches!(self.stage, Stage::Running { .. })
    }

    /// The galleries arrived for `ticket` (older answers are dropped).
    pub fn galleries_loaded(&mut self, ticket: u64, result: Result<Vec<Gallery>, String>) {
        if ticket == self.ticket {
            self.galleries = Some(result);
        }
    }

    /// A gallery's sections arrived (dropped unless it is still the chosen one).
    pub fn sections_loaded(&mut self, gallery: i64, result: Result<Vec<Section>, String>) {
        if self.chosen != Some(gallery) {
            return;
        }
        match result {
            Ok(s) => self.sections = Some(s),
            Err(e) => {
                // A gallery without its sections can still be uploaded to.
                log::warn!("could not list the sections of gallery {gallery}: {e}");
                self.sections = Some(Vec::new());
            }
        }
    }

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

    pub fn finished(&mut self, result: Result<GalleryOutcome, String>) {
        match result {
            Ok(o) => self.stage = Stage::Done(o),
            Err(e) => {
                self.stage = Stage::Form;
                self.error = Some(e);
            }
        }
    }

    /// The run the form describes, or the sentence to show.
    pub fn run(&self) -> Result<GalleryRun, &'static str> {
        let gallery = self.chosen.ok_or(CHOOSE)?;
        Ok(GalleryRun {
            gallery,
            section: self.section,
            size: if self.long_edge && self.pixels > 0 {
                Size::LongEdge(self.pixels)
            } else {
                Size::Original
            },
            quality: self.quality.clamp(1, 100),
        })
    }

    /// Picks `gallery` (and asks for its sections).
    pub fn choose(&mut self, gallery: i64) -> GalleryAction {
        if self.chosen == Some(gallery) {
            return GalleryAction::None;
        }
        self.chosen = Some(gallery);
        self.section = None;
        self.sections = None;
        GalleryAction::Sections(gallery)
    }

    pub fn show(&mut self, ctx: &egui::Context) -> GalleryAction {
        let mut action = GalleryAction::None;
        let _ = egui::Modal::new(egui::Id::unique("gallery-export")).show(ctx, |ui| {
            ui.set_max_width(480.0);
            ui.heading(title(self.ids.len()));
            ui.add_space(8.0);
            action = match self.stage.clone() {
                Stage::Form => self.form_ui(ui),
                Stage::Running { done, total } => self.running_ui(ui, done, total),
                Stage::Done(o) => done_ui(ui, &o),
            };
        });
        if action == GalleryAction::None && ctx.input(|i| i.key_pressed(Key::Escape)) {
            action = GalleryAction::Close;
        }
        action
    }

    fn form_ui(&mut self, ui: &mut egui::Ui) -> GalleryAction {
        let mut action = GalleryAction::None;
        ui.horizontal(|ui| {
            let label = ui.label("Gallery");
            let field = ui
                .add(egui::TextEdit::singleline(&mut self.search).desired_width(240.0))
                .labelled_by(label.id);
            let entered = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            if ui.button(SEARCH).clicked() || entered {
                self.ticket += 1;
                self.galleries = None;
                action = GalleryAction::Search(self.ticket, self.search.clone());
            }
        });
        ui.add_space(4.0);
        let mut picked = None;
        egui::ScrollArea::vertical()
            .id_salt("gallery-list")
            .max_height(220.0)
            .show(ui, |ui| match &self.galleries {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new(LOADING).color(theme::MUTED));
                    });
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(theme::DANGER));
                }
                Some(Ok(list)) if list.is_empty() => {
                    ui.label(RichText::new(NO_GALLERIES).color(theme::MUTED));
                }
                Some(Ok(list)) => {
                    for g in list {
                        let text = match &g.event_date {
                            Some(date) => format!("{}  ·  {date}", g.title),
                            None => g.title.clone(),
                        };
                        if ui
                            .selectable_label(self.chosen == Some(g.id), text)
                            .clicked()
                        {
                            picked = Some(g.id);
                        }
                    }
                }
            });
        if let Some(id) = picked {
            action = self.choose(id);
        }
        if self.chosen.is_some() {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Section");
                match &self.sections {
                    None => {
                        ui.spinner();
                    }
                    Some(sections) => {
                        let shown = self
                            .section
                            .and_then(|id| sections.iter().find(|s| s.id == id))
                            .map_or(NO_SECTION, |s| s.name.as_str())
                            .to_owned();
                        let list = egui::ComboBox::from_id_salt("gallery-section")
                            .selected_text(shown)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.section, None, NO_SECTION);
                                for s in sections {
                                    ui.selectable_value(&mut self.section, Some(s.id), &s.name);
                                }
                            });
                        list.response.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::ComboBox, true, "Section")
                        });
                    }
                }
            });
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("Size");
            ui.radio_value(&mut self.long_edge, false, "Original");
            ui.radio_value(&mut self.long_edge, true, "Long edge");
            if self.long_edge {
                ui.add(
                    egui::DragValue::new(&mut self.pixels)
                        .range(256..=20_000)
                        .suffix(" px"),
                );
            }
        });
        ui.horizontal(|ui| {
            let label = ui.label("Quality");
            ui.add(egui::Slider::new(&mut self.quality, 1..=100))
                .labelled_by(label.id);
        });
        if let Some(e) = &self.error {
            ui.add_space(6.0);
            ui.label(RichText::new(e).color(theme::DANGER));
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let ready = self.chosen.is_some();
            if ui.add_enabled(ready, egui::Button::new(UPLOAD)).clicked() {
                match self.run() {
                    Ok(run) => action = GalleryAction::Start(run),
                    Err(e) => self.error = Some(e.to_owned()),
                }
            }
            if ui.button(CANCEL).clicked() {
                action = GalleryAction::Close;
            }
        });
        action
    }

    fn running_ui(&mut self, ui: &mut egui::Ui, done: usize, total: usize) -> GalleryAction {
        let mut action = GalleryAction::None;
        ui.add(egui::ProgressBar::new(done as f32 / total.max(1) as f32));
        ui.add_space(6.0);
        ui.label(format!("{} of {}", count(done), count(total)));
        ui.label(RichText::new(KEEPS_RUNNING).color(theme::MUTED).small());
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let stopping = self.stop.load(Ordering::SeqCst);
            if ui.add_enabled(!stopping, egui::Button::new(STOP)).clicked() {
                self.stop.store(true, Ordering::SeqCst);
            }
            if ui.button(CLOSE).clicked() {
                action = GalleryAction::Close;
            }
        });
        action
    }
}

fn done_ui(ui: &mut egui::Ui, o: &GalleryOutcome) -> GalleryAction {
    let mut action = GalleryAction::None;
    ui.label(done_text(o));
    if !o.failed.is_empty() {
        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .max_height(200.0)
            .show(ui, |ui| {
                for (file, error) in &o.failed {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(file).strong());
                        ui.label(RichText::new(error).color(theme::DANGER));
                    });
                }
            });
    }
    ui.add_space(12.0);
    if ui.button(DONE).clicked() {
        action = GalleryAction::Close;
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gallery(id: i64) -> Gallery {
        Gallery {
            id,
            title: format!("G{id}"),
            event_date: None,
            status: "published".into(),
        }
    }

    #[test]
    fn opening_asks_for_the_list_and_late_answers_are_dropped() {
        let (mut d, first) = GalleryDialog::new(vec![1, 2]);
        assert_eq!(first, GalleryAction::Search(1, String::new()));
        d.galleries_loaded(0, Ok(vec![gallery(9)]));
        assert!(d.galleries.is_none(), "an older ticket's answer is dropped");
        d.galleries_loaded(1, Ok(vec![gallery(5)]));
        assert_eq!(d.galleries.as_ref().unwrap().as_ref().unwrap().len(), 1);
    }

    #[test]
    fn choosing_asks_for_sections_and_the_run_follows_the_form() {
        let (mut d, _) = GalleryDialog::new(vec![1]);
        assert_eq!(d.run(), Err(CHOOSE));
        assert_eq!(d.choose(5), GalleryAction::Sections(5));
        assert_eq!(
            d.choose(5),
            GalleryAction::None,
            "the same one again asks nothing"
        );
        d.sections_loaded(9, Ok(vec![]));
        assert!(
            d.sections.is_none(),
            "another gallery's sections are dropped"
        );
        d.sections_loaded(
            5,
            Ok(vec![Section {
                id: 2,
                name: "Wedding".into(),
            }]),
        );
        d.section = Some(2);
        assert_eq!(
            d.run(),
            Ok(GalleryRun {
                gallery: 5,
                section: Some(2),
                size: Size::LongEdge(DEFAULT_EDGE),
                quality: DEFAULT_QUALITY,
            })
        );
        d.long_edge = false;
        assert_eq!(d.run().unwrap().size, Size::Original);
        // Another gallery forgets the section.
        d.choose(6);
        assert_eq!(d.section, None);
    }

    #[test]
    fn a_refused_run_goes_back_to_the_form_with_its_message() {
        let (mut d, _) = GalleryDialog::new(vec![1]);
        d.started();
        assert!(d.running());
        d.progress(1, 1);
        d.finished(Err("Sign in again.".into()));
        assert_eq!(d.stage, Stage::Form);
        assert_eq!(d.error.as_deref(), Some("Sign in again."));
    }
}
