//! One lensflow-ImageEditor at a time: an exclusive lock on `<config dir>/instance.lock`
//! (the operating system's own file lock, so it goes with the process, even a crashed one).
//! A second launch says the app is already open and exits. The lock is let go at the very
//! end of `on_exit`, just before a restart or an installer is started; a restarted process
//! (told by [`RESTARTED_ENV`]) waits up to [`RESTART_WAIT`] for it.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;
use std::time::{Duration, Instant};

/// What a second launch says (a native message box) before it exits.
pub const ALREADY_OPEN: &str = "lensflow-ImageEditor is already open.";
/// The lock file's name inside the config dir.
pub const LOCK_FILE: &str = "instance.lock";
/// Set on a restarted process (after a device reset or an AppImage update): the old one may
/// still be finishing its exit, so the lock is waited for.
pub const RESTARTED_ENV: &str = "LENSFLOW_RESTARTED";
/// How long a restarted process waits for the old one's lock.
pub const RESTART_WAIT: Duration = Duration::from_secs(10);

/// The held lock; it is let go by [`Guard::release`], or when the process ends.
#[derive(Debug)]
pub struct Guard {
    file: File,
}

impl Guard {
    /// Lets the lock go (the end of `on_exit`, before a restart or installer starts).
    pub fn release(self) {
        if let Err(e) = self.file.unlock() {
            log::warn!("could not release the instance lock: {e}");
        }
    }
}

/// Why the lock was not taken.
#[derive(Debug)]
pub enum Refused {
    /// Another lensflow-ImageEditor holds it.
    AlreadyRunning,
    /// The lock file could not be made or locked (the app runs without the lock).
    Io(std::io::Error),
}

/// Takes the lock, or says why not, at once.
pub fn acquire(config_dir: &Path) -> Result<Guard, Refused> {
    std::fs::create_dir_all(config_dir).map_err(Refused::Io)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(config_dir.join(LOCK_FILE))
        .map_err(Refused::Io)?;
    match file.try_lock() {
        Ok(()) => Ok(Guard { file }),
        Err(TryLockError::WouldBlock) => Err(Refused::AlreadyRunning),
        Err(TryLockError::Error(e)) => Err(Refused::Io(e)),
    }
}

/// Takes the lock, retrying for up to `wait` while another process holds it.
pub fn acquire_waiting(config_dir: &Path, wait: Duration) -> Result<Guard, Refused> {
    let end = Instant::now() + wait;
    loop {
        match acquire(config_dir) {
            Err(Refused::AlreadyRunning) if Instant::now() < end => {
                std::thread::sleep(Duration::from_millis(100));
            }
            other => return other,
        }
    }
}

/// How long this launch waits for the lock: [`RESTART_WAIT`] when it is a restart.
pub fn wait_for_launch() -> Duration {
    if std::env::var_os(RESTARTED_ENV).is_some() {
        RESTART_WAIT
    } else {
        Duration::ZERO
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_fails_while_first_held() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        let first = acquire(&config).expect("the first launch takes the lock");
        assert!(config.join(LOCK_FILE).exists());
        assert!(matches!(acquire(&config), Err(Refused::AlreadyRunning)));
        drop(first);
    }

    #[test]
    fn released_lock_can_be_taken_again() {
        let dir = tempfile::tempdir().unwrap();
        let first = acquire(dir.path()).unwrap();
        first.release();
        let second = acquire(dir.path()).expect("free again after release");
        // A restarted process waits for the old one to let go.
        let config = dir.path().to_path_buf();
        let held = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            second.release();
        });
        let began = Instant::now();
        let third = acquire_waiting(&config, Duration::from_secs(10)).expect("taken once free");
        assert!(began.elapsed() >= Duration::from_millis(200), "it waited");
        held.join().unwrap();
        drop(third);
        // Without the wait, a held lock is refused at once.
        let fourth = acquire(dir.path()).unwrap();
        let began = Instant::now();
        assert!(matches!(
            acquire_waiting(dir.path(), Duration::from_millis(200)),
            Err(Refused::AlreadyRunning)
        ));
        assert!(began.elapsed() < Duration::from_secs(5));
        drop(fourth);
    }
}
