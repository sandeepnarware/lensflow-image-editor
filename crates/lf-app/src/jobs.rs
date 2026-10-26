//! Background work: one serial worker thread per [`Lane`], answering through one channel
//! of [`Msg`] values that the UI drains every frame.

use crate::account::{Generation, Token};
use crate::catalog_ext::{Imported, Opened, Pool};
use crate::develop::open::OpenedPhoto;
use crate::device_reset;
use crate::dialogs::batch::BatchProgress;
use crate::dialogs::match_reference::MatchStep;
use crate::export_job::ExportOutcome;
use crate::library::import_ui::{Chosen, Purpose};
use crate::presets_sync::{SyncError, SyncReport};
use crate::update::DownloadProgress;
use eframe::egui;
use lf_account::AccountError;
use lf_account::auth::{SignedIn, User};
use lf_account::entitlement::Decision;
use lf_catalog::import::{Progress, Scan};
use lf_catalog::{Folder, PhotoRow};
use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// A worker thread; jobs on one lane run one at a time, in the order spawned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    /// Sign-in and entitlement calls.
    Account,
    /// Opening and decoding photos.
    Open,
    /// Disk work: imports, sidecars, exports.
    Io,
    /// Quick catalogue reads (folders, a folder's photos), so they never wait behind an
    /// import or an export.
    Query,
}

impl Lane {
    /// The worker thread's name: `lf-account`, `lf-open`, `lf-io`, `lf-query`.
    pub fn thread_name(self) -> &'static str {
        match self {
            Lane::Account => "lf-account",
            Lane::Open => "lf-open",
            Lane::Io => "lf-io",
            Lane::Query => "lf-query",
        }
    }
}

/// What a job sends back to the UI. Later screens add their own variants.
#[derive(Debug)]
#[cfg_attr(
    test,
    derive(strum_macros::EnumDiscriminants),
    strum_discriminants(derive(strum_macros::EnumIter, Hash))
)]
pub enum Msg {
    /// The job named `name` panicked; `message` is the panic's text.
    JobFailed { name: String, message: String },
    /// At launch: the stored token, if there is one. Account answers carry the
    /// [`Generation`] they were asked in; the app drops answers from an older one.
    Resumed(Generation, Option<Token>),
    /// `auth/login` answered (`TwoFactorRequired` means ask for the code); a token that
    /// came back is already stored.
    SignedIn(Generation, Result<SignedIn, AccountError>),
    /// `auth/me` answered (at launch, with the stored token).
    Me(Generation, Result<User, AccountError>),
    /// The entitlement gate decided.
    Gate(Generation, Decision),
    /// The account lane cleared stored credentials (sign-out, studio switch, a 401).
    Cleared,
    /// The installer download moved on or finished.
    Download(DownloadProgress),
    /// A folder was chosen (picker or typed path) for `purpose`: the folder, `None` when the
    /// picker was cancelled, or why a typed path cannot be used. Answers whose `ticket` is
    /// not the latest are ignored.
    FolderChosen {
        purpose: Purpose,
        ticket: u64,
        result: Result<Option<Chosen>, String>,
    },
    /// The catalogue at `root` was opened, with its thumbnail pool and frame cache.
    CatalogueOpened {
        root: PathBuf,
        result: Result<Box<Opened>, String>,
    },
    /// A thumbnail pool built again after a device error.
    PoolReady(Result<Pool, String>),
    /// A folder's photos were marked missing after the user confirmed it.
    MarkedMissing {
        folder_id: i64,
        result: Result<usize, String>,
    },
    /// A folder was scanned for photos (picked, or rescanned).
    Scanned { folder: PathBuf, scan: Scan },
    /// An import moved on.
    ImportProgress(Progress),
    /// An import finished.
    ImportDone(Result<Imported, String>),
    /// The catalogue's folders, with their photo counts (`root`: the catalogue asked).
    Folders {
        root: PathBuf,
        result: Result<Vec<Folder>, String>,
    },
    /// A folder's photos, in capture order.
    Photos {
        /// The catalogue asked (a list for one left behind is dropped).
        root: PathBuf,
        folder_id: i64,
        result: Result<Vec<PhotoRow>, String>,
    },
    /// A folder was removed from the catalogue.
    FolderRemoved {
        folder_id: i64,
        result: Result<(), String>,
    },
    /// A photo opened for Develop (or why it cannot be), for the request of `generation`;
    /// answers from an older request are dropped.
    Opened {
        photo_id: i64,
        generation: u64,
        result: Result<Box<OpenedPhoto>, String>,
    },
    /// A neighbour was decoded into the frame cache (`Ok(false)`: skipped).
    Preopened {
        photo_id: i64,
        result: Result<bool, String>,
    },
    /// The answer to the device-reset message box.
    DeviceReset(device_reset::Choice),
    /// The `Io` lane wrote the waiting sidecars (the first error, if any).
    SidecarWritten(Result<(), String>),
    /// A studio preset sync ended (asked in `Generation`; older answers are dropped by the
    /// session, the library re-reads its preset list either way).
    PresetsSynced(Generation, Result<SyncReport, SyncError>),
    /// "Apply to N photos" moved on (or ended: `finished`).
    BatchProgress(BatchProgress),
    /// "Match a reference…": the picker, the fit's progress and answer, the apply's. A
    /// sign-out found during a fit (`MatchStep::SignedOut`) goes on to the session.
    Match(MatchStep),
    /// An export to disk moved on: `done` photos handled of `total`.
    ExportProgress { done: usize, total: usize },
    /// An export to disk ended (finished or stopped), or why it was refused (the sentence to
    /// show).
    ExportDone(Result<ExportOutcome, String>),

    #[cfg(test)]
    Probe(&'static str),
}

type Work = Box<dyn FnOnce() -> Msg + Send>;

struct Job {
    name: String,
    work: Work,
}

/// The three lanes and the channel they answer on.
///
/// [`Jobs::shutdown`] (called from `App::on_exit`) stops every lane from accepting work; each
/// lane finishes the job it is running, and the jobs still queued are discarded without
/// running (each logged by name at warn). It waits for the lanes until one shared deadline
/// (`timeout` after the call, so each lane is bounded by it) and logs any lane that is still
/// busy, which is then abandoned. Results produced during shutdown are dropped silently.
/// Dropping `Jobs` without `shutdown` does the same without waiting.
pub struct Jobs {
    lanes: Vec<Worker>,
    stopping: Arc<AtomicBool>,
    results: Receiver<Msg>,
    reporter: Reporter,
}

/// Lets a running job send messages before it finishes (progress).
#[derive(Clone)]
pub struct Reporter {
    results: Sender<Msg>,
    ctx: egui::Context,
}

impl Reporter {
    /// Sends `msg` to the UI and wakes it; dropped silently once the app has gone.
    pub fn send(&self, msg: Msg) {
        if self.results.send(msg).is_ok() {
            self.ctx.request_repaint();
        }
    }
}

/// Lets a running job queue work on another lane (a match's AI step asks the account lane
/// for a fresh entitlement and waits for the answer). It holds the lanes' queues open, so a
/// job keeps it only while it runs; after shutdown it queues nothing.
#[derive(Clone)]
pub struct Spawner {
    queues: Vec<(Lane, Sender<Job>)>,
    stopping: Arc<AtomicBool>,
}

impl Spawner {
    /// Queues `f` on `lane`, as [`Jobs::spawn`].
    pub fn spawn(&self, lane: Lane, name: &str, f: impl FnOnce() -> Msg + Send + 'static) {
        let queue = self.queues.iter().find(|(l, _)| *l == lane).map(|(_, q)| q);
        let job = Job {
            name: name.to_owned(),
            work: Box::new(f),
        };
        if self.stopping.load(Ordering::SeqCst) || queue.is_none_or(|q| q.send(job).is_err()) {
            log::warn!(
                "the {} worker has stopped; job \"{name}\" was dropped",
                lane.thread_name()
            );
        }
    }
}

struct Worker {
    lane: Lane,
    /// `None` once the lane stops accepting work.
    queue: Option<Sender<Job>>,
    thread: Option<JoinHandle<()>>,
}

impl Jobs {
    /// Starts the three lanes; each finished job wakes `ctx`.
    pub fn new(ctx: egui::Context) -> Jobs {
        let (results_tx, results) = mpsc::channel();
        let stopping = Arc::new(AtomicBool::new(false));
        let lanes = [Lane::Account, Lane::Open, Lane::Io, Lane::Query]
            .into_iter()
            .map(|lane| start_lane(lane, results_tx.clone(), ctx.clone(), stopping.clone()))
            .collect();
        Jobs {
            lanes,
            stopping,
            results,
            reporter: Reporter {
                results: results_tx.clone(),
                ctx: ctx.clone(),
            },
        }
    }

    /// Queues `f` on `lane`. The closure gets nothing but what it captures.
    pub fn spawn(&self, lane: Lane, name: &str, f: impl FnOnce() -> Msg + Send + 'static) {
        let job = Job {
            name: name.to_owned(),
            work: Box::new(f),
        };
        let queue = self
            .lanes
            .iter()
            .find(|w| w.lane == lane)
            .and_then(|w| w.queue.as_ref());
        if queue.is_none_or(|queue| queue.send(job).is_err()) {
            log::warn!(
                "the {} worker has stopped; job \"{name}\" was dropped",
                lane.thread_name()
            );
        }
    }

    /// A handle a job can capture to report progress while it runs.
    pub fn reporter(&self) -> Reporter {
        self.reporter.clone()
    }

    /// A handle a job can capture to queue work on another lane.
    pub fn spawner(&self) -> Spawner {
        Spawner {
            queues: self
                .lanes
                .iter()
                .filter_map(|w| w.queue.clone().map(|q| (w.lane, q)))
                .collect(),
            stopping: self.stopping.clone(),
        }
    }

    /// Every message sent since the last call, without waiting.
    pub fn drain(&self) -> Vec<Msg> {
        self.results.try_iter().collect()
    }

    /// Stops the lanes as described on [`Jobs`] and waits up to `timeout` for their running
    /// jobs. Returns the lanes still busy at the deadline (abandoned, not joined).
    pub fn shutdown(&mut self, timeout: Duration) -> Vec<Lane> {
        self.stop();
        let deadline = Instant::now() + timeout;
        let mut abandoned = Vec::new();
        for worker in &mut self.lanes {
            let Some(thread) = worker.thread.take() else {
                continue;
            };
            while !thread.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            if thread.is_finished() {
                let _ = thread.join();
            } else {
                log::warn!(
                    "the {} worker did not finish its job within {timeout:?}; abandoning it",
                    worker.lane.thread_name()
                );
                abandoned.push(worker.lane);
            }
        }
        abandoned
    }

    /// Stops accepting work and tells the workers to discard what is queued.
    fn stop(&mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        for worker in &mut self.lanes {
            worker.queue = None;
        }
    }
}

impl Drop for Jobs {
    fn drop(&mut self) {
        self.stop();
    }
}

fn start_lane(
    lane: Lane,
    results: Sender<Msg>,
    ctx: egui::Context,
    stopping: Arc<AtomicBool>,
) -> Worker {
    let (queue, jobs) = mpsc::channel::<Job>();
    let thread = std::thread::Builder::new()
        .name(lane.thread_name().to_owned())
        .spawn(move || {
            for Job { name, work } in jobs {
                if stopping.load(Ordering::SeqCst) {
                    log::warn!("shutting down: job \"{name}\" was discarded without running");
                    continue;
                }
                let msg = catch_unwind(AssertUnwindSafe(work)).unwrap_or_else(|panic| {
                    let message = panic_message(panic.as_ref());
                    log::error!("job \"{name}\" panicked: {message}");
                    Msg::JobFailed { name, message }
                });
                // The receiver may be gone during shutdown; the result is then dropped.
                if results.send(msg).is_ok() {
                    ctx.request_repaint();
                }
            }
        })
        .expect("spawning a worker thread");
    Worker {
        lane,
        queue: Some(queue),
        thread: Some(thread),
    }
}

fn panic_message(panic: &(dyn Any + Send)) -> String {
    if let Some(text) = panic.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = panic.downcast_ref::<String>() {
        text.clone()
    } else {
        "the job stopped unexpectedly".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(msg: &Msg) -> String {
        match msg {
            Msg::Probe(text) => (*text).to_owned(),
            Msg::JobFailed { name, message } => format!("{name} failed: {message}"),
            other => format!("{other:?}"),
        }
    }

    fn drain_until(jobs: &Jobs, count: usize) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut got = Vec::new();
        while got.len() < count && Instant::now() < deadline {
            got.extend(jobs.drain().iter().map(label));
            std::thread::sleep(Duration::from_millis(5));
        }
        got
    }

    #[test]
    fn jobs_delivers_results_in_order_per_lane_and_survives_a_panic() {
        let jobs = Jobs::new(egui::Context::default());
        assert!(jobs.drain().is_empty());
        jobs.spawn(Lane::Io, "first", || panic!("disk on fire"));
        jobs.spawn(Lane::Io, "second", || {
            assert_eq!(std::thread::current().name(), Some("lf-io"));
            Msg::Probe("second")
        });
        jobs.spawn(Lane::Io, "third", || Msg::Probe("third"));

        let got = drain_until(&jobs, 3);
        assert_eq!(got, ["first failed: disk on fire", "second", "third"]);
        assert!(jobs.drain().is_empty());

        jobs.spawn(Lane::Io, "progress", {
            let reporter = jobs.reporter();
            move || {
                reporter.send(Msg::Probe("halfway"));
                Msg::Probe("done")
            }
        });
        assert_eq!(drain_until(&jobs, 2), ["halfway", "done"]);
    }

    #[test]
    fn a_job_can_queue_work_on_another_lane_and_wait_for_it() {
        let jobs = Jobs::new(egui::Context::default());
        let spawner = jobs.spawner();
        jobs.spawn(Lane::Open, "asks", move || {
            let (tx, rx) = mpsc::channel();
            spawner.spawn(Lane::Account, "answers", move || {
                tx.send(std::thread::current().name().map(str::to_owned))
                    .unwrap();
                Msg::Probe("answered")
            });
            let from = rx.recv_timeout(Duration::from_secs(10)).unwrap();
            assert_eq!(from.as_deref(), Some("lf-account"));
            Msg::Probe("asked")
        });
        let mut got = drain_until(&jobs, 2);
        got.sort();
        assert_eq!(got, ["answered", "asked"]);
    }

    #[test]
    fn each_lane_runs_on_its_own_named_thread() {
        let jobs = Jobs::new(egui::Context::default());
        for lane in [Lane::Account, Lane::Open, Lane::Io, Lane::Query] {
            jobs.spawn(lane, lane.thread_name(), move || {
                assert_eq!(std::thread::current().name(), Some(lane.thread_name()));
                Msg::Probe(lane.thread_name())
            });
        }
        let mut got = drain_until(&jobs, 4);
        got.sort();
        assert_eq!(got, ["lf-account", "lf-io", "lf-open", "lf-query"]);
    }

    #[test]
    fn shutdown_finishes_the_running_job_and_discards_the_queue() {
        let mut jobs = Jobs::new(egui::Context::default());
        let first_done = Arc::new(AtomicBool::new(false));
        let others_ran = Arc::new(AtomicBool::new(false));
        let (started_tx, started) = mpsc::channel();
        {
            let first_done = first_done.clone();
            jobs.spawn(Lane::Io, "slow", move || {
                started_tx.send(()).unwrap();
                std::thread::sleep(Duration::from_millis(200));
                first_done.store(true, Ordering::SeqCst);
                Msg::Probe("slow")
            });
        }
        for name in ["queued 1", "queued 2"] {
            let others_ran = others_ran.clone();
            jobs.spawn(Lane::Io, name, move || {
                others_ran.store(true, Ordering::SeqCst);
                Msg::Probe("queued")
            });
        }
        started.recv_timeout(Duration::from_secs(10)).unwrap();

        let begun = Instant::now();
        let abandoned = jobs.shutdown(Duration::from_secs(2));
        assert!(begun.elapsed() < Duration::from_secs(2));
        assert!(abandoned.is_empty());
        assert!(
            first_done.load(Ordering::SeqCst),
            "the running job finished"
        );
        assert!(!others_ran.load(Ordering::SeqCst), "queued jobs never ran");

        // Nothing is accepted after shutdown.
        jobs.spawn(Lane::Io, "late", || Msg::Probe("late"));
        std::thread::sleep(Duration::from_millis(50));
        assert!(!jobs.drain().iter().any(|m| matches!(m, Msg::Probe("late"))));
    }

    #[test]
    fn shutdown_abandons_a_job_that_never_ends_at_the_bound() {
        let mut jobs = Jobs::new(egui::Context::default());
        let release = Arc::new(AtomicBool::new(false));
        let (started_tx, started) = mpsc::channel();
        {
            let release = release.clone();
            jobs.spawn(Lane::Open, "endless", move || {
                started_tx.send(()).unwrap();
                while !release.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Msg::Probe("endless")
            });
        }
        started.recv_timeout(Duration::from_secs(10)).unwrap();

        let begun = Instant::now();
        let abandoned = jobs.shutdown(Duration::from_millis(300));
        let took = begun.elapsed();
        assert_eq!(abandoned, vec![Lane::Open]);
        assert!(
            took >= Duration::from_millis(300),
            "waited for the bound ({took:?})"
        );
        assert!(
            took < Duration::from_secs(5),
            "returned at the bound ({took:?})"
        );
        release.store(true, Ordering::SeqCst);
    }
}

/// Who handles each message, and one of each, for the tests that check every message reaches
/// its owner (`session_flow`, `library`).
#[cfg(test)]
pub(crate) mod owners {
    use super::*;
    use crate::update::DownloadProgress;
    use std::collections::HashSet;
    use std::path::Path;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Owner {
        Session,
        Library,
        /// `App::handle` itself, before the library and the session see it.
        App,
        /// Both look at it: the library resets its own waits and passes it on.
        Shared,
        Test,
    }

    /// Exhaustive on purpose: a new `Msg` variant does not compile until it is placed here.
    /// Then add one to [`every_message`] (`every_message_holds_each_variant_once` compares it
    /// with every variant strum lists, and fails until it is there); the owner's test then
    /// feeds it to the owner.
    pub fn owner(msg: &Msg) -> Owner {
        match msg {
            Msg::JobFailed { .. } | Msg::PresetsSynced(..) => Owner::Shared,
            Msg::Match(MatchStep::SignedOut(..)) => Owner::Session,
            Msg::Resumed(..)
            | Msg::SignedIn(..)
            | Msg::Me(..)
            | Msg::Gate(..)
            | Msg::Cleared
            | Msg::Download(..) => Owner::Session,
            Msg::FolderChosen { .. }
            | Msg::CatalogueOpened { .. }
            | Msg::PoolReady(..)
            | Msg::MarkedMissing { .. }
            | Msg::Scanned { .. }
            | Msg::ImportProgress(..)
            | Msg::ImportDone(..)
            | Msg::Folders { .. }
            | Msg::Photos { .. }
            | Msg::FolderRemoved { .. }
            | Msg::Opened { .. }
            | Msg::Preopened { .. }
            | Msg::SidecarWritten(..)
            | Msg::BatchProgress(..)
            | Msg::Match(..)
            | Msg::ExportProgress { .. }
            | Msg::ExportDone(..) => Owner::Library,
            Msg::DeviceReset(..) => Owner::App,
            Msg::Probe(..) => Owner::Test,
        }
    }

    /// One of every variant: account answers at `generation`; paths under `dir`. None of
    /// them reaches the network, the keychain or a real catalogue from a fresh session or
    /// library.
    pub fn every_message(generation: Generation, dir: &Path) -> Vec<Msg> {
        use crate::library::import_ui::Purpose;
        use lf_catalog::import::{Progress, Scan};
        vec![
            Msg::JobFailed {
                name: "test".into(),
                message: "boom".into(),
            },
            Msg::Resumed(generation, None),
            Msg::SignedIn(generation, Err(AccountError::Network("offline".into()))),
            Msg::Me(generation, Err(AccountError::Server(400, "no".into()))),
            Msg::Gate(generation, Decision::ChooseStudio),
            Msg::Cleared,
            Msg::Download(DownloadProgress::Progress { done: 1, total: 2 }),
            Msg::FolderChosen {
                purpose: Purpose::AddFolder,
                ticket: 99,
                result: Ok(None),
            },
            Msg::CatalogueOpened {
                root: dir.join("elsewhere"),
                result: Err("no".into()),
            },
            Msg::PoolReady(Err("no".into())),
            Msg::MarkedMissing {
                folder_id: 1,
                result: Err("no".into()),
            },
            Msg::Scanned {
                folder: dir.join("gone"),
                scan: Scan {
                    candidates: Vec::new(),
                    root_readable: false,
                },
            },
            Msg::ImportProgress(Progress { done: 0, total: 1 }),
            Msg::ImportDone(Err("no".into())),
            Msg::Folders {
                root: dir.to_path_buf(),
                result: Err("no".into()),
            },
            Msg::Photos {
                root: dir.to_path_buf(),
                folder_id: 1,
                result: Err("no".into()),
            },
            Msg::FolderRemoved {
                folder_id: 1,
                result: Err("no".into()),
            },
            Msg::Opened {
                photo_id: 1,
                generation: 99,
                result: Err("stale".into()),
            },
            Msg::Preopened {
                photo_id: 1,
                result: Ok(false),
            },
            Msg::SidecarWritten(Err("disk full".into())),
            Msg::PresetsSynced(
                generation,
                Err(crate::presets_sync::SyncError::Catalog("no".into())),
            ),
            Msg::BatchProgress(BatchProgress {
                done: 1,
                total: 2,
                failed: Vec::new(),
                finished: true,
            }),
            Msg::Match(MatchStep::Progress {
                run: 99,
                text: "stale".into(),
            }),
            Msg::ExportProgress { done: 1, total: 2 },
            Msg::ExportDone(Err("no".into())),
            Msg::DeviceReset(device_reset::Choice::Quit),
            Msg::Probe("probe"),
        ]
    }

    /// `every_message` filtered to `owners`.
    pub fn messages_owned_by(owners: &[Owner], generation: Generation, dir: &Path) -> Vec<Msg> {
        every_message(generation, dir)
            .into_iter()
            .filter(|m| owners.contains(&owner(m)))
            .collect()
    }

    #[test]
    fn every_message_holds_each_variant_once() {
        use strum::IntoEnumIterator as _;
        let all = every_message(0, Path::new("."));
        let listed: HashSet<MsgDiscriminants> = all.iter().map(MsgDiscriminants::from).collect();
        assert_eq!(listed.len(), all.len(), "a variant is listed twice");
        let every: HashSet<MsgDiscriminants> = MsgDiscriminants::iter().collect();
        assert_eq!(
            listed, every,
            "every_message must hold one of each Msg variant (Probe included)"
        );
    }
}
