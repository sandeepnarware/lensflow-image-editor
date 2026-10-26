//! After the graphics device is lost, egui's own drawing may fail too, so the in-window
//! banner may never be seen. A native message box (on its own thread, so it shows whatever
//! state the window is in) asks to restart or quit; "Restart" starts this executable again
//! and closes the window.

use crate::develop::debounce::SidecarSaver;
use crate::jobs::{Msg, Reporter};
use rfd::{MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The message box's text.
pub const MESSAGE: &str = "The graphics device was reset. lensflow-ImageEditor needs to restart.";
pub const RESTART: &str = "Restart";
pub const QUIT: &str = "Quit";

/// How long the dialog's thread waits for the window to act on the choice before it ends
/// the process itself (the window's loop may be gone with the device).
pub const UNRESPONSIVE_AFTER: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Restart,
    Quit,
}

/// The choice a dialog answer stands for: the "Restart" button (or OK where the system
/// dialog cannot name its buttons) restarts; anything else quits.
pub fn choice(result: &MessageDialogResult) -> Choice {
    match result {
        MessageDialogResult::Ok | MessageDialogResult::Yes => Choice::Restart,
        MessageDialogResult::Custom(label) if label == RESTART => Choice::Restart,
        _ => Choice::Quit,
    }
}

/// The text shown: Windows' plain message box cannot name its buttons, so it says which is
/// which.
pub fn description() -> String {
    if cfg!(windows) {
        format!("{MESSAGE} Choose OK to restart, or Cancel to quit.")
    } else {
        MESSAGE.to_owned()
    }
}

/// What the dialog's thread does before it ends the process itself (the window never took
/// the answer, so `on_exit` will not run): a running import stops after its batch, and the
/// waiting sidecars are written.
#[derive(Clone, Default)]
pub struct ForceExit {
    pub stop_import: Option<Arc<AtomicBool>>,
    pub saver: Option<SidecarSaver>,
}

impl ForceExit {
    pub fn prepare(&self) {
        if let Some(stop) = &self.stop_import {
            stop.store(true, Ordering::SeqCst);
        }
        if let Some(saver) = &self.saver
            && let Err(e) = saver.flush()
        {
            log::error!("an edit file could not be written before ending the process: {e}");
        }
    }
}

/// Waits up to `timeout` for the window to take the answer (`answered`, set at the top of
/// `App::device_reset`); true when it did.
pub fn wait_for_answer(answered: &AtomicBool, timeout: Duration) -> bool {
    let waited = std::time::Instant::now();
    loop {
        if answered.load(Ordering::SeqCst) {
            return true;
        }
        if waited.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The app's side of quitting: the device-reset answer (`answered` tells the dialog's
/// thread the window is alive, so it never ends the process while the UI runs, even when
/// the window stays open), the instance lock, and what starts once `on_exit` has shut the
/// jobs down and flushed: a restart, or a downloaded installer.
#[derive(Debug, Default)]
pub struct Exit {
    pub answered: Arc<AtomicBool>,
    pub restart_on_exit: bool,
    /// The single-instance lock, let go just before anything is started.
    pub lock: Option<crate::single_instance::Guard>,
    /// A verified installer to hand over to the system after the app has let go.
    pub installer: Option<PathBuf>,
}

impl Exit {
    /// The window took `choice`.
    pub fn answer(&mut self, choice: Choice) {
        self.answered.store(true, Ordering::SeqCst);
        self.restart_on_exit = choice == Choice::Restart;
    }

    /// The very end of `on_exit`: lets the instance lock go, then starts the installer (it
    /// replaces this app, which is exiting) or, when a restart was asked for, the app again.
    pub fn finish(&mut self) {
        if let Some(lock) = self.lock.take() {
            lock.release();
        }
        if let Some(installer) = self.installer.take() {
            if let Err(e) = crate::update::launch(&installer) {
                crate::startup::tell(
                    rfd::MessageLevel::Error,
                    &crate::update::launch_failed_text(&installer, &e),
                );
            }
        } else if self.restart_on_exit {
            log::info!("restarting lensflow-ImageEditor");
            if let Err(e) = restart() {
                log::error!("could not restart lensflow-ImageEditor: {e}");
            }
        }
    }
}

/// Shows the message box on a thread of its own and sends the answer as
/// `Msg::DeviceReset`. When the window has not taken the answer (`answered`) within
/// [`UNRESPONSIVE_AFTER`], its loop is taken to be gone: the thread runs
/// [`ForceExit::prepare`], restarts (if asked) and ends the process.
pub fn ask(reporter: Reporter, answered: Arc<AtomicBool>, force: ForceExit) {
    let spawned = std::thread::Builder::new()
        .name("lf-device-reset".into())
        .spawn(move || {
            let result = MessageDialog::new()
                .set_level(MessageLevel::Error)
                .set_title("lensflow-ImageEditor")
                .set_description(description())
                .set_buttons(MessageButtons::OkCancelCustom(RESTART.into(), QUIT.into()))
                .show();
            let choice = choice(&result);
            log::info!("device reset: the user chose {choice:?}");
            reporter.send(Msg::DeviceReset(choice));
            if wait_for_answer(&answered, UNRESPONSIVE_AFTER) {
                return;
            }
            log::error!("the window did not respond after the device reset; ending the process");
            force.prepare();
            if choice == Choice::Restart
                && let Err(e) = restart()
            {
                log::error!("could not restart: {e}");
            }
            std::process::exit(1);
        });
    if let Err(e) = spawned {
        log::error!("could not show the device-reset message: {e}");
    }
}

/// The command that starts this app again with the same arguments: the AppImage itself
/// when running from one (`$APPIMAGE`; `current_exe()` points inside its temporary mount,
/// which goes away with this process), else this executable.
pub fn restart_command() -> std::io::Result<Command> {
    restart_command_for(std::env::var_os("APPIMAGE").as_deref())
}

/// [`restart_command`] for the AppImage at `appimage`, if any. The new process is told it
/// is a restart, so it waits for this one's instance lock.
pub fn restart_command_for(appimage: Option<&std::ffi::OsStr>) -> std::io::Result<Command> {
    let program = match appimage.filter(|a| !a.is_empty()) {
        Some(appimage) => PathBuf::from(appimage),
        None => std::env::current_exe()?,
    };
    let mut command = Command::new(program);
    command.args(std::env::args_os().skip(1));
    command.env(crate::single_instance::RESTARTED_ENV, "1");
    Ok(command)
}

/// Starts this executable again (the caller then closes the window).
pub fn restart() -> std::io::Result<()> {
    restart_command()?.spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_runs_this_executable_and_restart_is_the_default_button() {
        let command = restart_command().unwrap();
        assert_eq!(
            Path::new(command.get_program()),
            std::env::current_exe().unwrap()
        );
        assert_eq!(choice(&MessageDialogResult::Ok), Choice::Restart);
        assert_eq!(
            choice(&MessageDialogResult::Custom(RESTART.into())),
            Choice::Restart
        );
        assert_eq!(
            choice(&MessageDialogResult::Custom(QUIT.into())),
            Choice::Quit
        );
        assert_eq!(choice(&MessageDialogResult::Cancel), Choice::Quit);
        assert!(description().starts_with(MESSAGE));
    }

    #[test]
    fn appimage_restart_uses_the_appimage_env() {
        let appimage = std::ffi::OsStr::new("/home/ravi/Apps/lensflow-ImageEditor.AppImage");
        let command = restart_command_for(Some(appimage)).unwrap();
        assert_eq!(
            command.get_program(),
            appimage,
            "not the temporary mount's binary"
        );
        let restarted = command
            .get_envs()
            .any(|(k, v)| k == crate::single_instance::RESTARTED_ENV && v.is_some());
        assert!(restarted, "the new process waits for this one's lock");
        let command = restart_command_for(None).unwrap();
        assert_eq!(
            Path::new(command.get_program()),
            std::env::current_exe().unwrap()
        );
    }

    #[test]
    fn the_answer_flag_not_closing_keeps_the_process_alive() {
        // Ruling 1: the window took the answer (even if it stays open, e.g. a failed
        // restart): the thread stands down at once.
        let answered = Arc::new(AtomicBool::new(false));
        let setter = answered.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            setter.store(true, Ordering::SeqCst);
        });
        let began = std::time::Instant::now();
        assert!(wait_for_answer(&answered, Duration::from_secs(10)));
        assert!(began.elapsed() < Duration::from_secs(5));
        t.join().unwrap();
        // Never taken: it gives up at the bound.
        assert!(!wait_for_answer(
            &AtomicBool::new(false),
            Duration::from_millis(100)
        ));

        let mut exit = Exit::default();
        exit.answer(Choice::Quit);
        assert!(exit.answered.load(Ordering::SeqCst));
        assert!(!exit.restart_on_exit);
        // Ruling 3: a restart is only remembered; on_exit starts it after the flush.
        let mut exit = Exit::default();
        exit.answer(Choice::Restart);
        assert!(exit.answered.load(Ordering::SeqCst) && exit.restart_on_exit);
    }

    #[test]
    fn a_forced_exit_stops_the_import_and_writes_waiting_sidecars() {
        use crate::develop::debounce::PendingWrite;
        let dir = tempfile::tempdir().unwrap();
        let photo = dir.path().join("a.jpg");
        let saver = SidecarSaver::new();
        let mut settings = lf_settings::EditSettings::default_settings();
        settings.exposure = 0.7;
        let json = serde_json::json!({ "v": 2, "settings": settings.to_value() });
        let seed = dir.path().join("seed.json");
        std::fs::write(&seed, json.to_string()).unwrap();
        let sidecar = lf_catalog::sidecar::read(&seed).unwrap().unwrap();
        saver.push(
            PendingWrite::new(1, &photo, sidecar),
            std::time::Instant::now(),
        );
        let stop = Arc::new(AtomicBool::new(false));
        let force = ForceExit {
            stop_import: Some(stop.clone()),
            saver: Some(saver),
        };
        force.prepare();
        assert!(stop.load(Ordering::SeqCst), "the import was told to stop");
        let written = lf_catalog::sidecar::read(&lf_catalog::sidecar::path_for(&photo))
            .unwrap()
            .expect("the waiting sidecar was written");
        assert_eq!(written.settings.exposure, 0.7);
    }

    use std::path::Path;
}
