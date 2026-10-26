//! The account and session orchestration: who is signed in, the account-lane jobs, the
//! generation guard on their answers, the studio choice, the 4-hour gate clock and the
//! installer download. `App` feeds it messages and screen actions and reads the result;
//! the session's rules live in `session.rs`, the account calls in `account.rs`.

use crate::account::{self, Credentials, Generation, Token};
use crate::app::VERSION;
use crate::jobs::{Jobs, Lane, Msg};
use crate::prefs::{Dirs, InstanceChoice, Prefs};
use crate::screens::{Action, View};
use crate::session::{GateClock, Session, StudioChoice, choose_studio};
use crate::toast::{Level, Toasts};
use crate::update::{self, DownloadProgress, UpdateError};
use eframe::egui;
use lf_account::auth::{SignedIn, StudioSummary, User};
use lf_account::entitlement::{Cache, Decision};
use lf_account::{AccountError, Client, Instance, LockReason};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

/// What the flow needs from the app around it while it handles a message or an action.
pub struct Env<'a> {
    pub jobs: &'a Jobs,
    pub toasts: &'a mut Toasts,
    pub prefs: &'a mut Prefs,
    pub dirs: &'a Dirs,
    pub ctx: &'a egui::Context,
}

impl Env<'_> {
    fn save_prefs(&self) {
        if let Err(e) = self.prefs.save(&self.dirs.config) {
            log::error!("could not save preferences: {e}");
        }
    }
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

/// The session and the account behind it.
pub struct SessionFlow {
    pub session: Session,
    account: AccountState,
}

impl SessionFlow {
    pub fn new(prefs: &Prefs, config_dir: &Path) -> SessionFlow {
        SessionFlow {
            session: Session::signing_in(),
            account: AccountState::connect(prefs.instance, config_dir, 0),
        }
    }

    /// The gate's last answer: AI, preset sync and gallery export read it.
    pub fn gate(&self) -> Option<&Decision> {
        self.account.decision.as_ref()
    }

    /// Checks the entitlement now (before an AI call or an export); the answer arrives as
    /// `Msg::Gate` and updates [`SessionFlow::gate`].
    pub fn refresh_now(&mut self, jobs: &Jobs, reason: &str) {
        log::info!("checking the entitlement before {reason}");
        self.spawn_gate(jobs);
    }

    /// The facts the screens show about the account.
    pub fn view(&self, logo: Option<egui::TextureId>, prefs: &Prefs) -> View {
        View {
            logo,
            dev_server: prefs.instance == InstanceChoice::Dev,
            checking: self.account.checking,
            can_switch: self
                .account
                .user
                .as_ref()
                .is_some_and(|u| u.studios.len() > 1),
            download: self.account.download,
        }
    }

    /// Once per frame: the scheduled entitlement check, while a studio is open.
    pub fn tick(&mut self, now: Instant, jobs: &Jobs) {
        if matches!(self.session, Session::Active { .. })
            && self.account.checking.is_none()
            && GateClock::due(self.account.last_gate, now)
        {
            self.spawn_gate(jobs);
        }
    }

    /// Queues an account job; `f` gets the current generation and the credentials.
    fn spawn_account(
        &self,
        jobs: &Jobs,
        name: &str,
        f: impl FnOnce(Generation, &Credentials) -> Msg + Send + 'static,
    ) {
        let generation = self.account.generation;
        let creds = Arc::clone(&self.account.creds);
        jobs.spawn(Lane::Account, name, move || f(generation, &creds));
    }

    /// At launch: read the stored token on the account lane; with one, ask who it belongs
    /// to (`Msg::Resumed`), otherwise the sign-in form.
    pub fn resume(&mut self, jobs: &Jobs) {
        self.account.checking = Some("Checking your account…");
        self.spawn_account(jobs, "resume", account::resume);
    }

    fn spawn_me(&mut self, jobs: &Jobs) {
        let Some(token) = self.account.token.as_deref() else {
            return;
        };
        let client = self.account.client.with_token(token);
        self.account.checking = Some("Checking your account…");
        self.spawn_account(jobs, "me", move |generation, creds| {
            account::me(generation, &client, creds)
        });
    }

    fn spawn_gate(&mut self, jobs: &Jobs) {
        let (Some(client), Some(studio)) = (self.account.studio_client(), &self.account.studio)
        else {
            return;
        };
        let studio_id = studio.id;
        self.account.checking = Some("Checking your studio…");
        self.spawn_account(jobs, "entitlement", move |generation, creds| {
            account::gate(generation, &client, creds, studio_id)
        });
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
    fn signed_out_by(&mut self, err: &AccountError, jobs: &Jobs) -> bool {
        if !matches!(err, AccountError::Unauthorized) {
            return false;
        }
        log::info!("the server signed this device out");
        self.account.sign_out_locally();
        self.spawn_account(jobs, "forget", |_, creds| account::forget(creds));
        let session = std::mem::replace(&mut self.session, Session::signing_in());
        self.session = session.on_unauthorized();
        true
    }

    /// The user's studios are known: use the remembered or only one, or ask.
    fn studios_known(&mut self, env: &mut Env) {
        let Some(user) = &self.account.user else {
            return;
        };
        match choose_studio(user, env.prefs.studio_slug.as_deref()) {
            StudioChoice::Chosen(studio) => self.enter_studio(studio, env),
            StudioChoice::Ask | StudioChoice::None => {
                self.session = Session::ChoosingStudio { user: user.clone() };
            }
        }
    }

    fn enter_studio(&mut self, studio: StudioSummary, env: &mut Env) {
        env.prefs.studio_slug = Some(studio.slug.clone());
        env.save_prefs();
        self.account.studio = Some(studio.clone());
        match Session::on_studio_chosen(&studio) {
            Some(session) => self.session = session,
            None => self.spawn_gate(env.jobs),
        }
    }

    /// `auth/me` could not be reached: carry on with the remembered studio and the cached
    /// entitlement (the gate's offline grace), when there are both.
    fn offline_studio(&self, prefs: &Prefs) -> Option<(User, StudioSummary)> {
        let slug = prefs.studio_slug.as_deref()?;
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

    /// Handles the account's messages; any other message is handed back.
    pub fn handle(&mut self, msg: Msg, env: &mut Env) -> Option<Msg> {
        match msg {
            Msg::JobFailed { name, message } => {
                self.account.checking = None;
                if let Session::SigningIn { busy, .. } = &mut self.session {
                    *busy = false;
                }
                env.toasts.push(
                    Level::Error,
                    format!("Something went wrong ({name}). {message}"),
                );
            }
            Msg::Resumed(generation, token) if self.is_current(generation) => {
                self.account.checking = None;
                if let Some(Token(token)) = token {
                    self.account.token = Some(token);
                    self.spawn_me(env.jobs);
                }
            }
            Msg::SignedIn(generation, result) if self.is_current(generation) => {
                self.signed_in(result, env)
            }
            Msg::Me(generation, result) if self.is_current(generation) => {
                self.me_answered(result, env)
            }
            Msg::Gate(generation, decision) if self.is_current(generation) => {
                self.gate_decided(decision, env)
            }
            Msg::Resumed(..) | Msg::SignedIn(..) | Msg::Me(..) | Msg::Gate(..) | Msg::Cleared => {}
            Msg::Download(progress) => self.download(progress, env),
            other => return Some(other),
        }
        None
    }

    fn signed_in(&mut self, result: Result<SignedIn, AccountError>, env: &mut Env) {
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
                self.studios_known(env);
            }
            Err(AccountError::TwoFactorRequired) => {
                *two_factor = true;
                *error = None;
            }
            Err(e) => *error = Some(account::sign_in_error(&e)),
        }
    }

    fn me_answered(&mut self, result: Result<User, AccountError>, env: &mut Env) {
        self.account.checking = None;
        match result {
            Ok(user) => {
                self.account.offline_user = false;
                self.account.user = Some(user);
                self.studios_known(env);
            }
            Err(e) if self.signed_out_by(&e, env.jobs) => {}
            Err(e) if account::is_transient(&e) => match self.offline_studio(env.prefs) {
                Some((user, studio)) => {
                    self.account.offline_user = true;
                    self.account.user = Some(user);
                    self.account.studio = Some(studio);
                    self.spawn_gate(env.jobs);
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

    fn gate_decided(&mut self, decision: Decision, env: &mut Env) {
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
            env.prefs.studio_slug = None;
            env.save_prefs();
        } else if back_online {
            // Launched offline: now that the server answers, fetch the real studio list.
            self.spawn_me(env.jobs);
        }
    }

    fn download(&mut self, progress: DownloadProgress, env: &mut Env) {
        match progress {
            DownloadProgress::Progress { done, total } => {
                self.account.download = Some((done, total));
            }
            DownloadProgress::Finished(result) => {
                self.account.download = None;
                match result {
                    Ok(path) => match update::launch(&path) {
                        Ok(()) => env.ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                        Err(e) => env
                            .toasts
                            .push(Level::Error, format!("Could not start the installer. {e}")),
                    },
                    Err(UpdateError::Account(e)) if self.signed_out_by(&e, env.jobs) => {}
                    Err(e) => env.toasts.push(Level::Error, e.to_string()),
                }
            }
        }
    }

    /// A button or link on a session screen.
    pub fn act(&mut self, action: Action, env: &mut Env) {
        match action {
            Action::SignIn => self.submit_sign_in(env.jobs),
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
                env.prefs.instance = match env.prefs.instance {
                    InstanceChoice::Prod => InstanceChoice::Dev,
                    InstanceChoice::Dev => InstanceChoice::Prod,
                };
                env.save_prefs();
                log::info!("server switched to {:?}", env.prefs.instance);
                let generation = self.account.generation + 1;
                self.account =
                    AccountState::connect(env.prefs.instance, &env.dirs.config, generation);
                self.session = Session::signing_in();
                self.resume(env.jobs);
            }
            Action::ChooseStudio(slug) => {
                let studio = self
                    .account
                    .user
                    .as_ref()
                    .and_then(|u| u.studios.iter().find(|s| s.slug == slug).cloned());
                if let Some(studio) = studio {
                    self.enter_studio(studio, env);
                }
            }
            Action::SwitchStudio => {
                // Drop any answer for the old studio, and clear its cached entitlement on
                // the account lane, after any check already queued there.
                self.account.new_generation();
                self.spawn_account(env.jobs, "forget entitlement", |_, creds| {
                    account::forget_entitlement(creds)
                });
                self.account.studio = None;
                self.account.decision = None;
                env.prefs.studio_slug = None;
                env.save_prefs();
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
                self.spawn_account(env.jobs, "sign out", move |_, creds| {
                    account::sign_out(client.as_ref(), creds)
                });
                env.prefs.studio_slug = None;
                env.save_prefs();
                self.session = Session::signing_in();
            }
            Action::Retry => {
                if self.account.token.is_none() {
                    self.resume(env.jobs);
                } else if self.account.user.is_none() {
                    self.spawn_me(env.jobs);
                } else if self.account.studio.is_some() {
                    self.spawn_gate(env.jobs);
                } else {
                    self.studios_known(env);
                }
            }
            Action::Update => self.start_update(env),
        }
    }

    fn submit_sign_in(&mut self, jobs: &Jobs) {
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
        self.spawn_account(jobs, "sign in", move |generation, creds| {
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

    fn start_update(&mut self, env: &Env) {
        if self.account.download.is_some() {
            return;
        }
        let Some(client) = self.account.studio_client() else {
            return;
        };
        let dest = env.dirs.cache.join("updates");
        let reporter = env.jobs.reporter();
        self.account.download = Some((0, 0));
        env.jobs.spawn(Lane::Io, "update", move || {
            let result = update::run(&client, update::platform(), &dest, |progress| {
                reporter.send(Msg::Download(progress));
            });
            Msg::Download(DownloadProgress::Finished(result))
        });
    }
}
