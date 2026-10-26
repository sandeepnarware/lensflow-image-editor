//! Background work: one serial worker thread per [`Lane`], answering through one channel
//! of [`Msg`] values that the UI drains every frame.

use crate::catalog_ext::{Imported, Opened, Pool};
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
}

impl Lane {
    /// The worker thread's name: `lf-account`, `lf-open`, `lf-io`.
    pub fn thread_name(self) -> &'static str {
        match self {
            Lane::Account => "lf-account",
            Lane::Open => "lf-open",
            Lane::Io => "lf-io",
        }
    }
}

/// What a job sends back to the UI. Later screens add their own variants.
#[derive(Debug)]
pub enum Msg {
    /// The job named `name` panicked; `message` is the panic's text.
    JobFailed { name: String, message: String },
    /// `auth/login` answered (`TwoFactorRequired` means ask for the code).
    SignedIn(Result<SignedIn, AccountError>),
    /// `auth/me` answered (at launch, with the stored token).
    Me(Result<User, AccountError>),
    /// The entitlement gate decided.
    Gate(Decision),
    /// `auth/logout` was sent (its answer does not matter).
    SignedOut,
    /// The installer download moved on or finished.
    Download(DownloadProgress),
    /// The catalogue-root picker closed: the folder chosen, and whether it is on a network
    /// drive.
    RootPicked(Option<(PathBuf, bool)>),
    /// The catalogue at `root` was opened, with its thumbnail pool and frame cache.
    CatalogueOpened {
        root: PathBuf,
        result: Result<Box<Opened>, String>,
    },
    /// A thumbnail pool built again after a device error.
    PoolReady(Result<Pool, String>),
    /// The "Add folder" picker closed without a folder.
    NothingPicked,
    /// A folder was scanned for photos (picked, or rescanned).
    Scanned { folder: PathBuf, scan: Scan },
    /// An import moved on.
    ImportProgress(Progress),
    /// An import finished.
    ImportDone(Result<Imported, String>),
    /// The catalogue's folders, with their photo counts.
    Folders(Result<Vec<Folder>, String>),
    /// A folder's photos, in capture order.
    Photos {
        folder_id: i64,
        result: Result<Vec<PhotoRow>, String>,
    },
    /// A folder was removed from the catalogue.
    FolderRemoved {
        folder_id: i64,
        result: Result<(), String>,
    },
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
        let lanes = [Lane::Account, Lane::Open, Lane::Io]
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
    fn each_lane_runs_on_its_own_named_thread() {
        let jobs = Jobs::new(egui::Context::default());
        for lane in [Lane::Account, Lane::Open, Lane::Io] {
            jobs.spawn(lane, lane.thread_name(), move || {
                assert_eq!(std::thread::current().name(), Some(lane.thread_name()));
                Msg::Probe(lane.thread_name())
            });
        }
        let mut got = drain_until(&jobs, 3);
        got.sort();
        assert_eq!(got, ["lf-account", "lf-io", "lf-open"]);
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
