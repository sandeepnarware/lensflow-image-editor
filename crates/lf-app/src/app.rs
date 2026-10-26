//! The `eframe::App`: owns the shared GPU, preferences, jobs, toasts and the session, and
//! draws the current screen. The session's rules live in `session.rs`, the screens in
//! `screens/`, the account calls in `account.rs`; this file wires them together.

use crate::account::{self, Credentials, Generation, Token};
use crate::jobs::{Jobs, Lane, Msg};
use crate::library::{self, Library};
use crate::prefs::{Dirs, InstanceChoice, Prefs};
use crate::screens::{self, Action, View};
use crate::session::{GateClock, Session, StudioChoice, choose_studio};
use crate::toast::{Level, Toasts};
use crate::update::{self, DownloadProgress, UpdateError};
use crate::{gpu, theme};
use eframe::egui::{self, RichText};
use lf_account::auth::{SignedIn, StudioSummary, User};
use lf_account::entitlement::{Cache, Decision};
use lf_account::{AccountError, Client, Instance, LockReason};
use lf_pipeline::Gpu;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const SOFTWARE_BANNER: &str = "No graphics driver found: previews will be slow.";
pub const DEVICE_LOST_BANNER: &str = "The graphics device was reset. Reopen the photo to continue.";

/// This build's version, sent to the server and compared with its releases.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct App {
    dirs: Dirs,
    /// The shared device, or why the window has none.
    gpu: Result<Arc<Gpu>, String>,
    prefs: Prefs,
    jobs: Jobs,
    toasts: Toasts,
    logo: Option<egui::TextureHandle>,
    lost_logged: bool,
    ctx: egui::Context,
    session: Session,
    account: AccountState,
    /// The catalogue, grid and filmstrip (none without a graphics device).
    library: Option<Library>,
}

/// Who is signed in, where, and what the account lane is doing.
struct AccountState {
    /// Shared with the account-lane jobs, which do every keychain and cache access.
    creds: Arc<Credentials>,
    /// The API client without a token.
    client: Client,
    /// Bumped on a studio switch, a sign-out, a 401 and a server switch; answers from an
    /// older generation are dropped.
    generation: Generation,
    /// Kept in memory so each call can carry it; never logged.
    token: Option<String>,
    user: Option<User>,
    studio: Option<StudioSummary>,
    /// The user and studio were rebuilt from the cache because `auth/me` was unreachable;
    /// `me` runs again after the first fresh gate answer.
    offline_user: bool,
    /// The gate's last answer, for the feature gates.
    decision: Option<Decision>,
    /// An account call is in flight; the text says which.
    checking: Option<&'static str>,
    last_gate: Option<Instant>,
    /// Installer download progress (bytes done, total).
    download: Option<(u64, u64)>,
}

impl AccountState {
    fn connect(choice: InstanceChoice, config_dir: &Path, generation: Generation) -> AccountState {
        let instance = match choice {
            InstanceChoice::Prod => Instance::Prod,
            InstanceChoice::Dev => Instance::Dev,
        };
        AccountState {
            creds: Arc::new(Credentials::open(&instance, config_dir)),
            client: Client::new(instance, VERSION),
            generation,
            token: None,
            user: None,
            studio: None,
            offline_user: false,
            decision: None,
            checking: None,
            last_gate: None,
            download: None,
        }
    }

    /// The client with the token and the chosen studio, once both are known.
    fn studio_client(&self) -> Option<Client> {
        let token = self.token.as_deref()?;
        let studio = self.studio.as_ref()?;
        Some(self.client.with_token(token).with_studio(&studio.slug))
    }

    /// Starts a new generation: answers to anything asked before are dropped.
    fn new_generation(&mut self) {
        self.generation += 1;
        self.checking = None;
    }

    fn sign_out_locally(&mut self) {
        self.new_generation();
        self.token = None;
        self.user = None;
        self.studio = None;
        self.offline_user = false;
        self.decision = None;
        self.last_gate = None;
    }
}

impl App {
    pub fn new(cc: &eframe::CreationContext, dirs: Dirs) -> App {
        theme::apply(&cc.egui_ctx);
        let gpu = gpu::attach(cc.wgpu_render_state.as_ref());
        if let Err(e) = &gpu {
            log::error!("{e}");
        }
        let prefs = Prefs::load(&dirs.config);
        let account = AccountState::connect(prefs.instance, &dirs.config, 0);
        let library = gpu
            .as_ref()
            .ok()
            .map(|gpu| Library::new(gpu.clone(), dirs.cache.join("frames"), &prefs));
        let mut app = App {
            library,
            gpu,
            prefs,
            jobs: Jobs::new(cc.egui_ctx.clone()),
            toasts: Toasts::default(),
            logo: load_logo(&cc.egui_ctx),
            lost_logged: false,
            ctx: cc.egui_ctx.clone(),
            session: Session::signing_in(),
            account,
            dirs,
        };
        app.resume();
        app
    }

    /// The gate's last answer: AI, preset sync and gallery export read it.
    pub fn gate(&self) -> Option<&Decision> {
        self.account.decision.as_ref()
    }

    /// Checks the entitlement now (before an AI call or an export); the answer arrives as
    /// `Msg::Gate` and updates [`App::gate`].
    pub fn refresh_now(&mut self, reason: &str) {
        log::info!("checking the entitlement before {reason}");
        self.spawn_gate();
    }

    /// Queues an account job; `f` gets the current generation and the credentials.
    fn spawn_account(
        &self,
        name: &str,
        f: impl FnOnce(Generation, &Credentials) -> Msg + Send + 'static,
    ) {
        let generation = self.account.generation;
        let creds = Arc::clone(&self.account.creds);
        self.jobs
            .spawn(Lane::Account, name, move || f(generation, &creds));
    }

    /// At launch: read the stored token on the account lane; with one, ask who it belongs
    /// to (`Msg::Resumed`), otherwise the sign-in form.
    fn resume(&mut self) {
        self.account.checking = Some("Checking your account…");
        self.spawn_account("resume", account::resume);
    }

    fn spawn_me(&mut self) {
        let Some(token) = self.account.token.as_deref() else {
            return;
        };
        let client = self.account.client.with_token(token);
        self.account.checking = Some("Checking your account…");
        self.spawn_account("me", move |generation, creds| {
            account::me(generation, &client, creds)
        });
    }

    fn spawn_gate(&mut self) {
        let (Some(client), Some(studio)) = (self.account.studio_client(), &self.account.studio)
        else {
            return;
        };
        let studio_id = studio.id;
        self.account.checking = Some("Checking your studio…");
        self.spawn_account("entitlement", move |generation, creds| {
            account::gate(generation, &client, creds, studio_id)
        });
    }

    fn save_prefs(&self) {
        if let Err(e) = self.prefs.save(&self.dirs.config) {
            log::error!("could not save preferences: {e}");
        }
    }

    /// The answer belongs to the current generation; older ones are dropped.
    fn is_current(&self, generation: Generation) -> bool {
        let current = generation == self.account.generation;
        if !current {
            log::debug!("dropping an account answer from an earlier session");
        }
        current
    }

    /// A 401 from any call: back to sign-in, and the token and cache are forgotten on the
    /// account lane. False for any other error.
    fn signed_out_by(&mut self, err: &AccountError) -> bool {
        if !matches!(err, AccountError::Unauthorized) {
            return false;
        }
        log::info!("the server signed this device out");
        self.account.sign_out_locally();
        self.spawn_account("forget", |_, creds| account::forget(creds));
        let session = std::mem::replace(&mut self.session, Session::signing_in());
        self.session = session.on_unauthorized();
        true
    }

    /// The user's studios are known: use the remembered or only one, or ask.
    fn studios_known(&mut self) {
        let Some(user) = &self.account.user else {
            return;
        };
        match choose_studio(user, self.prefs.studio_slug.as_deref()) {
            StudioChoice::Chosen(studio) => self.enter_studio(studio),
            StudioChoice::Ask | StudioChoice::None => {
                self.session = Session::ChoosingStudio { user: user.clone() };
            }
        }
    }

    fn enter_studio(&mut self, studio: StudioSummary) {
        self.prefs.studio_slug = Some(studio.slug.clone());
        self.save_prefs();
        self.account.studio = Some(studio.clone());
        match Session::on_studio_chosen(&studio) {
            Some(session) => self.session = session,
            None => self.spawn_gate(),
        }
    }

    /// `auth/me` could not be reached: carry on with the remembered studio and the cached
    /// entitlement (the gate's offline grace), when there are both.
    fn offline_studio(&self) -> Option<(User, StudioSummary)> {
        let slug = self.prefs.studio_slug.as_deref()?;
        let cached = Cache::new(self.account.creds.cache_dir()).load()?;
        let summary = serde_json::json!({
            "id": cached.studio.id, "name": cached.studio.name, "slug": slug,
        });
        let studio: StudioSummary = serde_json::from_value(summary.clone()).ok()?;
        let user = serde_json::from_value(serde_json::json!({
            "id": 0, "name": "", "email": "", "studios": [summary],
        }))
        .ok()?;
        Some((user, studio))
    }

    fn handle(&mut self, msg: Msg) {
        let mut env = library::Env {
            jobs: &self.jobs,
            toasts: &mut self.toasts,
            prefs: &mut self.prefs,
            config_dir: &self.dirs.config,
        };
        let msg = match &mut self.library {
            Some(lib) => match lib.handle(msg, &mut env) {
                Some(msg) => msg,
                None => return,
            },
            None => msg,
        };
        match msg {
            Msg::JobFailed { name, message } => {
                self.account.checking = None;
                if let Session::SigningIn { busy, .. } = &mut self.session {
                    *busy = false;
                }
                self.toasts.push(
                    Level::Error,
                    format!("Something went wrong ({name}). {message}"),
                );
            }
            Msg::Resumed(generation, token) if self.is_current(generation) => {
                self.account.checking = None;
                if let Some(Token(token)) = token {
                    self.account.token = Some(token);
                    self.spawn_me();
                }
            }
            Msg::SignedIn(generation, result) if self.is_current(generation) => {
                self.signed_in(result)
            }
            Msg::Me(generation, result) if self.is_current(generation) => self.me_answered(result),
            Msg::Gate(generation, decision) if self.is_current(generation) => {
                self.gate_decided(decision)
            }
            Msg::Resumed(..) | Msg::SignedIn(..) | Msg::Me(..) | Msg::Gate(..) | Msg::Cleared => {}
            Msg::Download(progress) => self.download(progress),
            // The library's own messages (handled above when there is a library).
            Msg::FolderChosen { .. }
            | Msg::CatalogueOpened { .. }
            | Msg::PoolReady(_)
            | Msg::MarkedMissing { .. }
            | Msg::Scanned { .. }
            | Msg::ImportProgress(_)
            | Msg::ImportDone(_)
            | Msg::Folders(_)
            | Msg::Photos { .. }
            | Msg::FolderRemoved { .. } => {}
            #[cfg(test)]
            Msg::Probe(_) => {}
        }
    }

    fn signed_in(&mut self, result: Result<SignedIn, AccountError>) {
        let Session::SigningIn {
            password,
            code,
            two_factor,
            busy,
            error,
            ..
        } = &mut self.session
        else {
            return;
        };
        *busy = false;
        match result {
            Ok(signed) => {
                // The account lane has already stored the token.
                password.clear();
                code.clear();
                self.account.token = Some(signed.token);
                self.account.user = Some(signed.user);
                self.studios_known();
            }
            Err(AccountError::TwoFactorRequired) => {
                *two_factor = true;
                *error = None;
            }
            Err(e) => *error = Some(account::sign_in_error(&e)),
        }
    }

    fn me_answered(&mut self, result: Result<User, AccountError>) {
        self.account.checking = None;
        match result {
            Ok(user) => {
                self.account.offline_user = false;
                self.account.user = Some(user);
                self.studios_known();
            }
            Err(e) if self.signed_out_by(&e) => {}
            Err(e) if account::is_transient(&e) => match self.offline_studio() {
                Some((user, studio)) => {
                    self.account.offline_user = true;
                    self.account.user = Some(user);
                    self.account.studio = Some(studio);
                    self.spawn_gate();
                }
                None => {
                    self.session = Session::Locked {
                        reason: LockReason::Offline,
                        message: format!("Could not reach LensFlow. {e}"),
                    };
                }
            },
            Err(e) => {
                self.session = Session::Locked {
                    reason: LockReason::Refused,
                    message: format!("Could not check your account. {e}"),
                };
            }
        }
    }

    fn gate_decided(&mut self, decision: Decision) {
        self.account.checking = None;
        self.account.last_gate = Some(Instant::now());
        let (Some(user), Some(studio)) = (&self.account.user, &self.account.studio) else {
            return;
        };
        let signs_out = matches!(decision, Decision::SignIn);
        let choose = matches!(decision, Decision::ChooseStudio);
        let back_online = self.account.offline_user && matches!(decision, Decision::Allow(_));
        let session = std::mem::replace(&mut self.session, Session::signing_in());
        // A `SignIn` decision: the account lane has already forgotten the credentials.
        self.session = session.on_decision(decision.clone(), user, studio, VERSION);
        self.account.decision = Some(decision);
        if signs_out {
            self.account.sign_out_locally();
        } else if choose {
            self.account.studio = None;
            self.prefs.studio_slug = None;
            self.save_prefs();
        } else if back_online {
            // Launched offline: now that the server answers, fetch the real studio list.
            self.spawn_me();
        }
    }

    fn download(&mut self, progress: DownloadProgress) {
        match progress {
            DownloadProgress::Progress { done, total } => {
                self.account.download = Some((done, total));
            }
            DownloadProgress::Finished(result) => {
                self.account.download = None;
                match result {
                    Ok(path) => match update::launch(&path) {
                        Ok(()) => self.ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                        Err(e) => self
                            .toasts
                            .push(Level::Error, format!("Could not start the installer. {e}")),
                    },
                    Err(UpdateError::Account(e)) if self.signed_out_by(&e) => {}
                    Err(e) => self.toasts.push(Level::Error, e.to_string()),
                }
            }
        }
    }

    fn act(&mut self, action: Action) {
        match action {
            Action::SignIn => self.submit_sign_in(),
            Action::StartOver => {
                let email = match &self.session {
                    Session::SigningIn { email, .. } => email.clone(),
                    _ => String::new(),
                };
                self.session = Session::signing_in();
                if let Session::SigningIn { email: field, .. } = &mut self.session {
                    *field = email;
                }
            }
            Action::ToggleInstance => {
                if matches!(self.session, Session::SigningIn { busy: true, .. }) {
                    return;
                }
                self.prefs.instance = match self.prefs.instance {
                    InstanceChoice::Prod => InstanceChoice::Dev,
                    InstanceChoice::Dev => InstanceChoice::Prod,
                };
                self.save_prefs();
                log::info!("server switched to {:?}", self.prefs.instance);
                let generation = self.account.generation + 1;
                self.account =
                    AccountState::connect(self.prefs.instance, &self.dirs.config, generation);
                self.session = Session::signing_in();
                self.resume();
            }
            Action::ChooseStudio(slug) => {
                let studio = self
                    .account
                    .user
                    .as_ref()
                    .and_then(|u| u.studios.iter().find(|s| s.slug == slug).cloned());
                if let Some(studio) = studio {
                    self.enter_studio(studio);
                }
            }
            Action::SwitchStudio => {
                // Drop any answer for the old studio, and clear its cached entitlement on
                // the account lane, after any check already queued there.
                self.account.new_generation();
                self.spawn_account("forget entitlement", |_, creds| {
                    account::forget_entitlement(creds)
                });
                self.account.studio = None;
                self.account.decision = None;
                self.prefs.studio_slug = None;
                self.save_prefs();
                if let Some(user) = &self.account.user {
                    self.session = Session::ChoosingStudio { user: user.clone() };
                }
            }
            Action::SignOut => {
                let client = self
                    .account
                    .token
                    .as_deref()
                    .map(|token| self.account.client.with_token(token));
                self.account.sign_out_locally();
                self.spawn_account("sign out", move |_, creds| {
                    account::sign_out(client.as_ref(), creds)
                });
                self.prefs.studio_slug = None;
                self.save_prefs();
                self.session = Session::signing_in();
            }
            Action::Retry => {
                if self.account.token.is_none() {
                    self.resume();
                } else if self.account.user.is_none() {
                    self.spawn_me();
                } else if self.account.studio.is_some() {
                    self.spawn_gate();
                } else {
                    self.studios_known();
                }
            }
            Action::Update => self.start_update(),
        }
    }

    fn submit_sign_in(&mut self) {
        let Session::SigningIn {
            email,
            password,
            code,
            two_factor,
            busy,
            error,
        } = &mut self.session
        else {
            return;
        };
        if *busy {
            return;
        }
        let email = email.trim().to_owned();
        if email.is_empty() || password.is_empty() {
            *error = Some("Enter your email and password.".to_owned());
            return;
        }
        if *two_factor && code.trim().is_empty() {
            *error = Some("Enter the code from your authenticator app.".to_owned());
            return;
        }
        *busy = true;
        *error = None;
        let password = password.clone();
        let code = two_factor.then(|| code.trim().to_owned());
        let client = self.account.client.clone();
        self.spawn_account("sign in", move |generation, creds| {
            account::sign_in(
                generation,
                &client,
                creds,
                &email,
                &password,
                code.as_deref(),
            )
        });
    }

    fn start_update(&mut self) {
        if self.account.download.is_some() {
            return;
        }
        let Some(client) = self.account.studio_client() else {
            return;
        };
        let dest = self.dirs.cache.join("updates");
        let reporter = self.jobs.reporter();
        self.account.download = Some((0, 0));
        self.jobs.spawn(Lane::Io, "update", move || {
            let result = update::run(&client, update::platform(), &dest, |progress| {
                reporter.send(Msg::Download(progress));
            });
            Msg::Download(DownloadProgress::Finished(result))
        });
    }

    fn view(&self) -> View {
        View {
            logo: self.logo.as_ref().map(|logo| logo.id()),
            dev_server: self.prefs.instance == InstanceChoice::Dev,
            checking: self.account.checking,
            can_switch: self
                .account
                .user
                .as_ref()
                .is_some_and(|u| u.studios.len() > 1),
            download: self.account.download,
        }
    }

    /// The signed-in screen until the library arrives.
    fn signed_in_placeholder(&self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() / 3.0);
            if let Some(logo) = &self.logo {
                ui.image(egui::load::SizedTexture::new(logo.id(), [96.0, 96.0]));
                ui.add_space(16.0);
            }
            if let Session::Active { studio, .. } = &self.session {
                ui.heading(format!("Signed in to {}.", studio.name));
            }
        });
    }

    /// The session's screen, or the chrome around the signed-in view.
    fn session_ui(&mut self, ui: &mut egui::Ui) {
        let view = self.view();
        let action = if matches!(self.session, Session::Active { .. }) {
            let top = screens::active_top(ui, &self.session, &view);
            let status = screens::status_bar(ui, &self.session, &view);
            match &mut self.library {
                Some(lib) => {
                    let mut env = library::Env {
                        jobs: &self.jobs,
                        toasts: &mut self.toasts,
                        prefs: &mut self.prefs,
                        config_dir: &self.dirs.config,
                    };
                    lib.ui(ui, &mut env);
                }
                None => {
                    egui::CentralPanel::default_margins()
                        .show(ui, |ui| self.signed_in_placeholder(ui));
                }
            }
            top.or(status)
        } else {
            egui::CentralPanel::default_margins()
                .show(ui, |ui| screens::show(ui, &mut self.session, &view))
                .inner
        };
        if let Some(action) = action {
            self.act(action);
        }
    }
}

fn banner(ui: &mut egui::Ui, id: &str, text: &str, colour: egui::Color32) {
    egui::Panel::top(id).show(ui, |ui| {
        ui.add_space(4.0);
        ui.label(RichText::new(text).color(colour));
        ui.add_space(4.0);
    });
}

fn load_logo(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let image = match image::load_from_memory(theme::LOGO_PNG) {
        Ok(image) => image.to_rgba8(),
        Err(e) => {
            log::warn!("could not decode the logo: {e}");
            return None;
        }
    };
    let size = [image.width() as usize, image.height() as usize];
    let pixels = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    Some(ctx.load_texture("logo", pixels, egui::TextureOptions::LINEAR))
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        for msg in self.jobs.drain() {
            self.handle(msg);
        }
        if matches!(self.session, Session::Active { .. }) {
            if self.account.checking.is_none()
                && GateClock::due(self.account.last_gate, Instant::now())
            {
                self.spawn_gate();
            }
            // The status bar's "Last checked" and the next scheduled check.
            ctx.request_repaint_after(Duration::from_secs(60));
        }
        if let Ok(gpu) = &self.gpu {
            if let Some(error) = gpu.take_error() {
                log::error!("graphics error: {error}");
            }
            if gpu.is_lost() && !self.lost_logged {
                log::error!("the graphics device was lost");
                self.lost_logged = true;
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        match &self.gpu {
            Err(message) => {
                egui::CentralPanel::default_margins().show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(ui.available_height() / 3.0);
                        ui.heading("lensflow-ImageEditor cannot start.");
                        ui.label(message.as_str());
                        ui.add_space(12.0);
                        if ui.button("Quit").clicked() {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                });
            }
            Ok(gpu) => {
                if gpu.is_lost() {
                    banner(ui, "device-lost", DEVICE_LOST_BANNER, theme::DANGER);
                } else if gpu.is_software() {
                    banner(ui, "software", SOFTWARE_BANNER, theme::WARNING);
                }
                self.session_ui(ui);
            }
        }
        self.toasts.show(ui.ctx());
    }

    fn on_exit(&mut self) {
        if let Some(lib) = &self.library {
            // A running import stops after the batch in progress (each batch commits).
            lib.stop_import();
        }
        self.jobs.shutdown(Duration::from_secs(10));
        self.save_prefs();
    }

    /// Only the window's size and position persist (in `window.ron`), not egui's memory.
    fn persist_egui_memory(&self) -> bool {
        false
    }
}
