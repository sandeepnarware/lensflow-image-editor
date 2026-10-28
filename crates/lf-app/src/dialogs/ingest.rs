//! "Import from card": copy the photos on a memory card (or any folder) into a folder of the
//! studio's, optionally renamed and backed up, then catalogue the copies. The card is only
//! read. The steps: choose the card (picker or typed path), which is scanned and planned on
//! the `Long` lane ([`lf_catalog::ingest::plan`]: capture dates, suspected duplicates); fill
//! in where the copies go; then the copy runs on the `Long` lane with its progress here
//! (Stop; closing the window leaves it running), and the library catalogues the new folder.

use crate::catalog_ext::count;
use crate::prefs::IngestPrefs;
use crate::theme;
use eframe::egui::{self, Key, RichText};
use lf_catalog::ingest::{IngestOutcome, IngestSpec, Planned, target_names};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub const TITLE: &str = "Import from a card";
pub const CHOOSE_CARD: &str = "Choose the card or folder…";
pub const CARD_PATH: &str = "Card or folder path";
pub const LOOK: &str = "Look for photos";
pub const LOOKING: &str = "Looking for photos…";
pub const NOTHING_FOUND: &str = "No photos were found there.";
pub const DESTINATION: &str = "Copy into";
pub const SUBFOLDER: &str = "Folder name";
pub const RENAME: &str = "Rename";
pub const PATTERN: &str = "Rename pattern";
pub const BACKUP: &str = "Also copy to a backup folder";
pub const BACKUP_PATH: &str = "Backup folder";
pub const SKIP_DUPLICATES: &str = "Skip photos already in the catalogue";
pub const CHOOSE: &str = "Choose…";
pub const STOP: &str = "Stop";
pub const CLOSE: &str = "Close";
pub const CANCEL: &str = "Cancel";
pub const KEEPS_RUNNING: &str = "Closing this window does not stop the copy.";
pub const NO_DESTINATION: &str = "Choose where the copies go.";
pub const NO_SUBFOLDER: &str = "Give the folder a name.";
pub const NO_BACKUP: &str = "Choose a backup folder, or turn the backup off.";
pub const SAME_FOLDER: &str = "The backup must be another folder than the copies.";
/// The rename pattern's tokens, said under it.
pub const TOKENS: &str = "{name} {seq} {date} {camera}";
pub const DEFAULT_PATTERN: &str = "{date}_{seq}";

/// "Import 120 photos".
pub fn start_label(n: usize) -> String {
    if n == 1 {
        "Import 1 photo".to_owned()
    } else {
        format!("Import {} photos", count(n))
    }
}

/// How a finished copy reads.
pub fn done_text(o: &IngestOutcome) -> String {
    let noun = |n: usize| if n == 1 { "photo" } else { "photos" };
    let mut text = format!("{} {} copied", count(o.copied), noun(o.copied));
    if o.duplicates > 0 {
        text.push_str(&format!("; {} already there", count(o.duplicates)));
    }
    if !o.failed.is_empty() {
        text.push_str(&format!("; {} could not be copied", count(o.failed.len())));
    }
    if o.backup_failed > 0 {
        text.push_str(&format!(
            "; {} without a backup copy",
            count(o.backup_failed)
        ));
    }
    if o.cancelled {
        text.push_str("; stopped");
    }
    text.push('.');
    text
}

/// Which folder a picker is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    Card,
    Destination,
    Backup,
}

/// Where the dialog is.
#[derive(Debug, Clone, PartialEq)]
pub enum Stage {
    /// Choosing the card.
    Source,
    Scanning,
    Form,
    Running {
        done: usize,
        total: usize,
    },
    Done(IngestOutcome),
}

/// What the dialog asks of the library this frame.
#[derive(Debug, Clone, PartialEq)]
pub enum IngestAction {
    None,
    /// Open a folder picker for this purpose, answering to this ticket.
    Pick(Pick, u64),
    /// Scan and plan this folder.
    Scan(PathBuf),
    Start(Box<IngestRun>),
    Close,
}

/// A copy to start: what, where, and the names (from [`target_names`]).
#[derive(Debug, Clone, PartialEq)]
pub struct IngestRun {
    pub spec: IngestSpec,
    pub plan: Vec<Planned>,
    pub names: Vec<Option<String>>,
}

pub struct IngestDialog {
    pub stage: Stage,
    pub stop: Arc<AtomicBool>,
    pub visible: bool,
    pub card: String,
    plan: Vec<Planned>,
    /// The folder the named folder is made in.
    pub destination: String,
    pub subfolder: String,
    pub rename: bool,
    pub pattern: String,
    pub backup_on: bool,
    pub backup: String,
    pub skip_duplicates: bool,
    /// The last ticket handed to a picker; older answers are dropped.
    ticket: u64,
    error: Option<String>,
}

impl IngestDialog {
    /// A new dialog, filled in as last time (the destination defaults to `default_to`).
    pub fn new(last: Option<&IngestPrefs>, default_to: Option<PathBuf>) -> IngestDialog {
        let last = last.cloned().unwrap_or_default();
        let shown = |p: Option<PathBuf>| p.map(|p| p.display().to_string()).unwrap_or_default();
        IngestDialog {
            stage: Stage::Source,
            stop: Arc::new(AtomicBool::new(false)),
            visible: true,
            card: String::new(),
            plan: Vec::new(),
            destination: shown(last.destination.or(default_to)),
            subfolder: String::new(),
            rename: last.rename,
            pattern: if last.pattern.trim().is_empty() {
                DEFAULT_PATTERN.to_owned()
            } else {
                last.pattern
            },
            backup_on: last.backup.is_some(),
            backup: shown(last.backup),
            skip_duplicates: last.skip_duplicates,
            ticket: 0,
            error: None,
        }
    }

    /// What to remember for next time.
    pub fn prefs(&self) -> IngestPrefs {
        let path = |s: &str| (!s.trim().is_empty()).then(|| PathBuf::from(s.trim()));
        IngestPrefs {
            destination: path(&self.destination),
            rename: self.rename,
            pattern: self.pattern.clone(),
            backup: if self.backup_on {
                path(&self.backup)
            } else {
                None
            },
            skip_duplicates: self.skip_duplicates,
        }
    }

    pub fn running(&self) -> bool {
        matches!(self.stage, Stage::Running { .. })
    }

    fn pick(&mut self, purpose: Pick) -> IngestAction {
        self.ticket += 1;
        IngestAction::Pick(purpose, self.ticket)
    }

    /// A picker answered (`None`: cancelled); a stale ticket is dropped. A chosen card is
    /// scanned at once.
    pub fn picked(&mut self, purpose: Pick, ticket: u64, path: Option<PathBuf>) -> IngestAction {
        let Some(path) = path.filter(|_| ticket == self.ticket) else {
            return IngestAction::None;
        };
        let shown = path.display().to_string();
        match purpose {
            Pick::Card => {
                self.card = shown;
                return self.scan();
            }
            Pick::Destination => self.destination = shown,
            Pick::Backup => self.backup = shown,
        }
        IngestAction::None
    }

    fn scan(&mut self) -> IngestAction {
        let card = self.card.trim();
        if card.is_empty() {
            return IngestAction::None;
        }
        self.error = None;
        self.stage = Stage::Scanning;
        IngestAction::Scan(PathBuf::from(card))
    }

    /// The card's photos, read and judged (or why they could not be).
    pub fn planned(&mut self, result: Result<Vec<Planned>, String>) {
        if self.stage != Stage::Scanning {
            return;
        }
        match result {
            Ok(plan) if plan.is_empty() => {
                self.error = Some(NOTHING_FOUND.to_owned());
                self.stage = Stage::Source;
            }
            Ok(plan) => {
                // Named for the shoot's first day, as Lightroom's "By date" would.
                if self.subfolder.trim().is_empty() {
                    self.subfolder = plan[0].date.clone();
                }
                self.plan = plan;
                self.stage = Stage::Form;
            }
            Err(e) => {
                self.error = Some(e);
                self.stage = Stage::Source;
            }
        }
    }

    pub fn started(&mut self) {
        self.stop.store(false, Ordering::SeqCst);
        self.stage = Stage::Running {
            done: 0,
            total: self.plan.len(),
        };
    }

    pub fn progress(&mut self, done: usize, total: usize) {
        if self.running() {
            self.stage = Stage::Running { done, total };
        }
    }

    pub fn finished(&mut self, result: Result<IngestOutcome, String>) {
        match result {
            Ok(o) => self.stage = Stage::Done(o),
            Err(e) => {
                self.error = Some(e);
                self.stage = Stage::Form;
            }
        }
    }

    /// The photos that will be copied.
    fn kept(&self) -> usize {
        self.plan
            .iter()
            .filter(|p| !(self.skip_duplicates && p.duplicate))
            .count()
    }

    /// The spec the form describes, or what is missing.
    pub fn spec(&self) -> Result<IngestSpec, &'static str> {
        let destination = self.destination.trim();
        if destination.is_empty() {
            return Err(NO_DESTINATION);
        }
        let subfolder = lf_catalog::export::safe_stem(self.subfolder.trim());
        let subfolder: String = subfolder
            .chars()
            .map(|c| if "\\/:*?\"<>|".contains(c) { '-' } else { c })
            .collect();
        if subfolder.trim().is_empty() {
            return Err(NO_SUBFOLDER);
        }
        let backup = if self.backup_on {
            let b = self.backup.trim();
            if b.is_empty() {
                return Err(NO_BACKUP);
            }
            Some(PathBuf::from(b).join(&subfolder))
        } else {
            None
        };
        let destination = PathBuf::from(destination).join(&subfolder);
        if backup.as_ref() == Some(&destination) {
            return Err(SAME_FOLDER);
        }
        Ok(IngestSpec {
            destination,
            rename: (self.rename && !self.pattern.trim().is_empty())
                .then(|| self.pattern.trim().to_owned()),
            backup,
            skip_duplicates: self.skip_duplicates,
        })
    }

    pub fn show(&mut self, ctx: &egui::Context) -> IngestAction {
        let mut action = IngestAction::None;
        let _ = egui::Modal::new(egui::Id::unique("ingest-dialog")).show(ctx, |ui| {
            ui.set_width(480.0);
            ui.heading(TITLE);
            ui.add_space(8.0);
            action = match self.stage.clone() {
                Stage::Source | Stage::Scanning => self.source_ui(ui),
                Stage::Form => self.form_ui(ui),
                Stage::Running { done, total } => self.running_ui(ui, done, total),
                Stage::Done(o) => done_ui(ui, &o),
            };
        });
        action
    }

    fn source_ui(&mut self, ui: &mut egui::Ui) -> IngestAction {
        let mut action = IngestAction::None;
        let scanning = self.stage == Stage::Scanning;
        ui.label(
            RichText::new("The photos are copied; the card is only read.").color(theme::MUTED),
        );
        ui.add_space(8.0);
        ui.add_enabled_ui(!scanning, |ui| {
            if ui.button(CHOOSE_CARD).clicked() {
                action = self.pick(Pick::Card);
            }
            ui.horizontal(|ui| {
                let typed = ui.add(
                    egui::TextEdit::singleline(&mut self.card)
                        .hint_text(CARD_PATH)
                        .desired_width(330.0),
                );
                named(&typed, CARD_PATH);
                let enter = typed.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                if ui.button(LOOK).clicked() || enter {
                    action = self.scan();
                }
            });
        });
        if scanning {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(LOOKING);
            });
        }
        if let Some(e) = &self.error {
            ui.label(RichText::new(e).color(theme::WARNING));
        }
        ui.add_space(12.0);
        if ui.button(CANCEL).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
            action = IngestAction::Close;
        }
        action
    }

    fn form_ui(&mut self, ui: &mut egui::Ui) -> IngestAction {
        let mut action = IngestAction::None;
        let duplicates = self.plan.iter().filter(|p| p.duplicate).count();
        ui.label(format!(
            "{} — {} {} found",
            self.card.trim(),
            count(self.plan.len()),
            if self.plan.len() == 1 {
                "photo"
            } else {
                "photos"
            }
        ));
        if duplicates > 0 {
            ui.label(
                RichText::new(format!(
                    "{} already in the catalogue (same name and size).",
                    count(duplicates)
                ))
                .color(theme::MUTED),
            );
            ui.checkbox(&mut self.skip_duplicates, SKIP_DUPLICATES);
        }
        ui.add_space(8.0);
        let folder_row = |ui: &mut egui::Ui, label: &str, text: &mut String, pick: Pick| {
            let mut clicked = None;
            ui.label(RichText::new(label).color(theme::MUTED));
            ui.horizontal(|ui| {
                let r = ui.add(egui::TextEdit::singleline(text).desired_width(360.0));
                named(&r, label);
                if ui.button(CHOOSE).clicked() {
                    clicked = Some(pick);
                }
            });
            clicked
        };
        let mut picked = folder_row(ui, DESTINATION, &mut self.destination, Pick::Destination);
        ui.label(RichText::new(SUBFOLDER).color(theme::MUTED));
        let r = ui.add(egui::TextEdit::singleline(&mut self.subfolder).desired_width(360.0));
        named(&r, SUBFOLDER);
        ui.add_space(6.0);
        ui.checkbox(&mut self.rename, RENAME);
        if self.rename {
            let r = ui.add(egui::TextEdit::singleline(&mut self.pattern).desired_width(360.0));
            named(&r, PATTERN);
            let example = self.spec().ok().and_then(|spec| {
                let first = self
                    .plan
                    .iter()
                    .find(|p| !(spec.skip_duplicates && p.duplicate))?;
                target_names(&spec, std::slice::from_ref(first))
                    .into_iter()
                    .next()
                    .flatten()
            });
            let mut hint = format!("Tokens: {TOKENS}");
            if let Some(e) = example {
                hint.push_str(&format!(" · e.g. {e}"));
            }
            ui.label(RichText::new(hint).color(theme::MUTED).small());
        }
        ui.add_space(6.0);
        ui.checkbox(&mut self.backup_on, BACKUP);
        if self.backup_on {
            picked = picked.or(folder_row(ui, BACKUP_PATH, &mut self.backup, Pick::Backup));
        }
        if let Some(p) = picked {
            action = self.pick(p);
        }
        if let Some(e) = &self.error {
            ui.label(RichText::new(e).color(theme::WARNING));
        }
        ui.add_space(12.0);
        let spec = self.spec();
        if let Err(why) = spec {
            ui.label(RichText::new(why).color(theme::MUTED));
        }
        let kept = self.kept();
        ui.horizontal(|ui| {
            let start = ui.add_enabled(
                spec.is_ok() && kept > 0,
                egui::Button::new(start_label(kept)),
            );
            if start.clicked()
                && let Ok(spec) = spec
            {
                let names = target_names(&spec, &self.plan);
                action = IngestAction::Start(Box::new(IngestRun {
                    spec,
                    plan: self.plan.clone(),
                    names,
                }));
            }
            if ui.button(CANCEL).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                action = IngestAction::Close;
            }
        });
        action
    }

    fn running_ui(&mut self, ui: &mut egui::Ui, done: usize, total: usize) -> IngestAction {
        let fraction = if total == 0 {
            0.0
        } else {
            done as f32 / total as f32
        };
        ui.add(egui::ProgressBar::new(fraction).text(format!(
            "Copying {} of {}",
            count(done),
            count(total)
        )));
        ui.label(RichText::new(KEEPS_RUNNING).color(theme::MUTED));
        ui.add_space(8.0);
        let mut action = IngestAction::None;
        ui.horizontal(|ui| {
            let stopping = self.stop.load(Ordering::SeqCst);
            if ui.add_enabled(!stopping, egui::Button::new(STOP)).clicked() {
                self.stop.store(true, Ordering::SeqCst);
            }
            if ui.button(CLOSE).clicked() {
                action = IngestAction::Close;
            }
        });
        action
    }
}

fn done_ui(ui: &mut egui::Ui, o: &IngestOutcome) -> IngestAction {
    ui.label(done_text(o));
    for (name, why) in o.failed.iter().take(8) {
        ui.label(RichText::new(format!("{name}: {why}")).color(theme::WARNING));
    }
    ui.add_space(12.0);
    if ui.button(CLOSE).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
        IngestAction::Close
    } else {
        IngestAction::None
    }
}

/// Names a text field for screen readers (and the tests).
fn named(response: &egui::Response, name: &str) {
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, name));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn planned(name: &str, duplicate: bool) -> Planned {
        Planned {
            from: PathBuf::from(format!("/card/{name}")),
            size: 10,
            date: "2026-10-28".into(),
            camera: None,
            duplicate,
        }
    }

    #[test]
    fn a_chosen_card_is_scanned_and_its_first_date_names_the_folder() {
        let mut d = IngestDialog::new(None, Some(PathBuf::from("/photos")));
        assert_eq!(
            d.destination,
            PathBuf::from("/photos").display().to_string()
        );
        let IngestAction::Pick(Pick::Card, ticket) = d.pick(Pick::Card) else {
            panic!()
        };
        // A stale answer is dropped.
        assert_eq!(
            d.picked(Pick::Card, ticket - 1, Some("/old".into())),
            IngestAction::None
        );
        assert_eq!(
            d.picked(Pick::Card, ticket, Some("/card".into())),
            IngestAction::Scan("/card".into())
        );
        assert_eq!(d.stage, Stage::Scanning);
        d.planned(Ok(vec![planned("a.nef", false), planned("b.nef", true)]));
        assert_eq!(d.stage, Stage::Form);
        assert_eq!(d.subfolder, "2026-10-28");
        assert_eq!(d.kept(), 1, "duplicates are skipped by default");
        let spec = d.spec().unwrap();
        assert_eq!(
            spec.destination,
            PathBuf::from("/photos").join("2026-10-28")
        );
        assert_eq!(spec.rename, None);
        assert!(spec.skip_duplicates);
    }

    #[test]
    fn the_form_says_what_is_missing() {
        let mut d = IngestDialog::new(None, None);
        d.planned(Ok(vec![planned("a.nef", false)]));
        d.stage = Stage::Form;
        assert_eq!(d.spec(), Err(NO_DESTINATION));
        d.destination = "/photos".into();
        d.subfolder = "  ".into();
        assert_eq!(d.spec(), Err(NO_SUBFOLDER));
        d.subfolder = "Sharma: wedding".into();
        d.backup_on = true;
        assert_eq!(d.spec(), Err(NO_BACKUP));
        d.backup = "/photos".into();
        assert_eq!(d.spec(), Err(SAME_FOLDER));
        d.backup = "/backup".into();
        d.rename = true;
        let spec = d.spec().unwrap();
        assert_eq!(
            spec.destination,
            PathBuf::from("/photos").join("Sharma- wedding")
        );
        assert_eq!(spec.rename.as_deref(), Some(DEFAULT_PATTERN));
        // Remembered for next time.
        let again = IngestDialog::new(Some(&d.prefs()), None);
        assert!(again.rename && again.backup_on);
        assert_eq!(again.backup, PathBuf::from("/backup").display().to_string());
    }

    #[test]
    fn an_empty_card_goes_back_to_choosing() {
        let mut d = IngestDialog::new(None, None);
        d.card = "/card".into();
        assert_eq!(d.scan(), IngestAction::Scan("/card".into()));
        d.planned(Ok(Vec::new()));
        assert_eq!(d.stage, Stage::Source);
        assert_eq!(d.error.as_deref(), Some(NOTHING_FOUND));
    }

    #[test]
    fn the_closing_line_says_what_happened() {
        let o = IngestOutcome {
            copied: 118,
            duplicates: 2,
            failed: vec![("x.nef".into(), "no".into())],
            backup_failed: 1,
            cancelled: false,
        };
        assert_eq!(
            done_text(&o),
            "118 photos copied; 2 already there; 1 could not be copied; 1 without a backup copy."
        );
    }
}
