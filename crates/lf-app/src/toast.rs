//! Short messages in the bottom-right corner. Errors stay until closed; the rest hide
//! after six seconds.

use crate::theme;
use eframe::egui::{self, Align2, Color32, RichText};

/// How long a non-error toast stays up, in seconds.
pub const SHOW_FOR: f64 = 6.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Success,
    Warning,
    Error,
}

impl Level {
    fn colour(self) -> Color32 {
        match self {
            Level::Info => theme::INK,
            Level::Success => theme::SUCCESS,
            Level::Warning => theme::WARNING,
            Level::Error => theme::DANGER,
        }
    }
}

struct Toast {
    id: u64,
    level: Level,
    text: String,
    /// When it was first drawn (egui time), so it hides six seconds after being seen.
    shown_at: Option<f64>,
}

#[derive(Default)]
pub struct Toasts {
    toasts: Vec<Toast>,
    next_id: u64,
}

impl Toasts {
    pub fn push(&mut self, level: Level, text: impl Into<String>) {
        self.toasts.push(Toast {
            id: self.next_id,
            level,
            text: text.into(),
            shown_at: None,
        });
        self.next_id += 1;
    }

    /// Draws the toasts over everything else, dropping expired and closed ones.
    pub fn show(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        self.toasts
            .retain(|t| t.level == Level::Error || t.shown_at.is_none_or(|at| now - at < SHOW_FOR));
        if self.toasts.is_empty() {
            return;
        }
        let mut closed = None;
        egui::Area::new(egui::Id::unique("toasts"))
            .anchor(Align2::RIGHT_BOTTOM, [-16.0, -16.0])
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_max_width(360.0);
                for toast in &mut self.toasts {
                    let shown_at = *toast.shown_at.get_or_insert(now);
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&toast.text).color(toast.level.colour()));
                            if toast.level == Level::Error && ui.small_button("Close").clicked() {
                                closed = Some(toast.id);
                            }
                        });
                    });
                    if toast.level != Level::Error {
                        let left = SHOW_FOR - (now - shown_at);
                        ctx.request_repaint_after(std::time::Duration::from_secs_f64(
                            left.max(0.0),
                        ));
                    }
                }
            });
        if let Some(id) = closed {
            self.toasts.retain(|t| t.id != id);
        }
    }
}
