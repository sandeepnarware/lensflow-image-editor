//! Background work: one serial worker thread per [`Lane`], answering through one channel
//! of [`Msg`] values that the UI drains every frame.

use eframe::egui;
use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, Receiver, Sender};

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
#[derive(Debug, PartialEq)]
pub enum Msg {
    /// The job named `name` panicked; `message` is the panic's text.
    JobFailed { name: String, message: String },
    #[cfg(test)]
    Probe(&'static str),
}

type Work = Box<dyn FnOnce() -> Msg + Send>;

struct Job {
    name: String,
    work: Work,
}

/// The three lanes and the channel they answer on. Dropping it lets each worker finish its
/// current job and stop.
pub struct Jobs {
    account: Sender<Job>,
    open: Sender<Job>,
    io: Sender<Job>,
    results: Receiver<Msg>,
}

impl Jobs {
    /// Starts the three lanes; each finished job wakes `ctx`.
    pub fn new(ctx: egui::Context) -> Jobs {
        let (results_tx, results) = mpsc::channel();
        let start = |lane: Lane| start_lane(lane, results_tx.clone(), ctx.clone());
        Jobs {
            account: start(Lane::Account),
            open: start(Lane::Open),
            io: start(Lane::Io),
            results,
        }
    }

    /// Queues `f` on `lane`. The closure gets nothing but what it captures.
    pub fn spawn(&self, lane: Lane, name: &str, f: impl FnOnce() -> Msg + Send + 'static) {
        let queue = match lane {
            Lane::Account => &self.account,
            Lane::Open => &self.open,
            Lane::Io => &self.io,
        };
        let job = Job {
            name: name.to_owned(),
            work: Box::new(f),
        };
        if queue.send(job).is_err() {
            log::error!(
                "the {} worker has stopped; job \"{name}\" was dropped",
                lane.thread_name()
            );
        }
    }

    /// Every message sent since the last call, without waiting.
    pub fn drain(&self) -> Vec<Msg> {
        self.results.try_iter().collect()
    }
}

fn start_lane(lane: Lane, results: Sender<Msg>, ctx: egui::Context) -> Sender<Job> {
    let (queue, jobs) = mpsc::channel::<Job>();
    std::thread::Builder::new()
        .name(lane.thread_name().to_owned())
        .spawn(move || {
            for Job { name, work } in jobs {
                let msg = catch_unwind(AssertUnwindSafe(work)).unwrap_or_else(|panic| {
                    let message = panic_message(panic.as_ref());
                    log::error!("job \"{name}\" panicked: {message}");
                    Msg::JobFailed { name, message }
                });
                if results.send(msg).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        })
        .expect("spawning a worker thread");
    queue
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
    use std::time::{Duration, Instant};

    fn drain_until(jobs: &Jobs, count: usize) -> Vec<Msg> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut got = Vec::new();
        while got.len() < count && Instant::now() < deadline {
            got.extend(jobs.drain());
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
        assert_eq!(
            got,
            vec![
                Msg::JobFailed {
                    name: "first".into(),
                    message: "disk on fire".into()
                },
                Msg::Probe("second"),
                Msg::Probe("third"),
            ]
        );
        assert!(jobs.drain().is_empty());
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
        got.sort_by_key(|m| format!("{m:?}"));
        assert_eq!(
            got,
            vec![
                Msg::Probe("lf-account"),
                Msg::Probe("lf-io"),
                Msg::Probe("lf-open")
            ]
        );
    }
}
