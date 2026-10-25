//! The screens before the editor (sign-in, studio choice, locks, updates) and the chrome
//! around it (account menu, read-only banner, status bar). Each screen draws from the
//! [`Session`] and returns what the user asked for as an [`Action`]; the app does the rest.

pub mod locked;
pub mod signin;
pub mod studio;
pub mod update;

use crate::session::{self, NO_PERMISSION_MESSAGE, READ_ONLY_BANNER, Session};
use crate::theme;
use eframe::egui::{self, RichText};
use lf_account::entitlement::Standing;
use std::time::{Duration, SystemTime};

/// Shown under the logo while the hidden instance switch points at dev.
pub const DEV_SERVER_NOTE: &str = "Using the development server.";

/// What the user asked for on a screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Submit the sign-in form (or its two-factor step).
    SignIn,
    /// Leave the two-factor step for a different account.
    StartOver,
    /// `Ctrl+Shift+D` on the sign-in screen: production ⇄ development server.
    ToggleInstance,
    /// Use the studio with this slug.
    ChooseStudio(String),
    SwitchStudio,
    SignOut,
    /// Check the entitlement (or the account) again.
    Retry,
    /// Download and install the newer release.
    Update,
}

/// What the screens need besides the session.
#[derive(Default, Clone)]
pub struct View {
    pub logo: Option<egui::TextureId>,
    /// The development server is in use.
    pub dev_server: bool,
    /// A call is in flight; the text says which (shown instead of the screen).
    pub checking: Option<&'static str>,
    /// The user has more than one studio.
    pub can_switch: bool,
    /// Installer download progress: bytes done and total.
    pub download: Option<(u64, u64)>,
}

/// The logo, top-centre, and the development-server note when it applies.
pub(crate) fn logo(ui: &mut egui::Ui, view: &View) {
    ui.add_space(48.0);
    if let Some(logo) = view.logo {
        ui.image(egui::load::SizedTexture::new(logo, [96.0, 96.0]));
    }
    if view.dev_server {
        ui.add_space(4.0);
        ui.label(RichText::new(DEV_SERVER_NOTE).color(theme::WARNING));
    }
    ui.add_space(24.0);
}

/// A 360 px card with the form's contents, left-aligned.
pub(crate) fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::group(ui.style())
        .fill(theme::SURFACE)
        .inner_margin(20.0)
        .corner_radius(8.0)
        .show(ui, |ui| {
            ui.set_width(360.0);
            ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), add)
                .inner
        })
        .inner
}

/// The installer download's progress bar, or an Update button.
pub(crate) fn update_button(ui: &mut egui::Ui, view: &View) -> Option<Action> {
    match view.download {
        Some((done, total)) => {
            let fraction = if total == 0 {
                0.0
            } else {
                done as f32 / total as f32
            };
            ui.add(
                egui::ProgressBar::new(fraction)
                    .desired_width(320.0)
                    .text("Downloading the update…"),
            );
            None
        }
        None => ui.button("Update").clicked().then_some(Action::Update),
    }
}

/// The screen for any session but `Active` (which the app draws around the editor).
pub fn show(ui: &mut egui::Ui, session: &mut Session, view: &View) -> Option<Action> {
    if let Some(text) = view
        .checking
        .filter(|_| !matches!(session, Session::Active { .. }))
    {
        ui.vertical_centered(|ui| {
            logo(ui, view);
            ui.spinner();
            ui.label(text);
        });
        return None;
    }
    match session {
        Session::SigningIn { .. } => signin::show(ui, session, view),
        Session::ChoosingStudio { user } => studio::show(ui, user, view),
        Session::NoPermission { studio } => {
            locked::show(ui, NO_PERMISSION_MESSAGE, Some(studio.as_str()), view)
        }
        Session::Locked { message, .. } => locked::show(ui, message, None, view),
        Session::UpdateRequired { latest } => update::show(ui, latest.as_ref(), view),
        Session::Active { .. } => None,
    }
}

/// The top bar (studio name and the account menu) and the read-only banner.
pub fn active_top(ui: &mut egui::Ui, session: &Session, view: &View) -> Option<Action> {
    let Session::Active {
        studio, read_only, ..
    } = session
    else {
        return None;
    };
    let mut action = None;
    egui::Panel::top("account-bar").show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&studio.name).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button("Account", |ui| {
                    if view.can_switch && ui.button("Switch studio").clicked() {
                        action = Some(Action::SwitchStudio);
                    }
                    if ui.button("Sign out").clicked() {
                        action = Some(Action::SignOut);
                    }
                });
            });
        });
    });
    if *read_only {
        egui::Panel::top("read-only").show(ui, |ui| {
            ui.add_space(4.0);
            ui.label(RichText::new(READ_ONLY_BANNER).color(theme::WARNING));
            ui.add_space(4.0);
        });
    }
    action
}

/// The 24 px status bar: studio, standing, when the entitlement was last checked, and the
/// update offer.
pub fn status_bar(ui: &mut egui::Ui, session: &Session, view: &View) -> Option<Action> {
    let Session::Active {
        studio,
        entitlement,
        stale,
        read_only,
        update,
    } = session
    else {
        return None;
    };
    let mut action = None;
    egui::Panel::bottom("status-bar")
        .exact_size(24.0)
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(&studio.name);
                ui.separator();
                ui.label(standing_text(entitlement.standing, *read_only));
                ui.separator();
                let age = stale.unwrap_or_else(|| {
                    SystemTime::now()
                        .duration_since(entitlement.checked_at)
                        .unwrap_or(Duration::ZERO)
                });
                let checked = RichText::new(session::last_checked(age));
                ui.label(if stale.is_some() {
                    checked.color(theme::WARNING)
                } else {
                    checked.color(theme::MUTED)
                });
                if let Some(release) = update {
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| match view.download {
                            Some(_) => {
                                update_button(ui, view);
                            }
                            None => {
                                let offer =
                                    format!("Version {} is ready — Update", release.version);
                                if ui.link(offer).clicked() {
                                    action = Some(Action::Update);
                                }
                            }
                        },
                    );
                }
            });
        });
    action
}

fn standing_text(standing: Standing, read_only: bool) -> &'static str {
    match standing {
        Standing::Active if !read_only => "Active",
        _ => "Read-only",
    }
}
