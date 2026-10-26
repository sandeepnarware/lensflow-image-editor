//! After the graphics device is lost, egui's own drawing may fail too, so the in-window
//! banner may never be seen. A native message box (on its own thread, so it shows whatever
//! state the window is in) asks to restart or quit; "Restart" starts this executable again
//! and closes the window.

use crate::jobs::{Msg, Reporter};
use rfd::{MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
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

/// Shows the message box on a thread of its own and sends the answer as
/// `Msg::DeviceReset`. When the window has not started closing (`closing`) within
/// [`UNRESPONSIVE_AFTER`] of the answer, its loop is taken to be gone: the thread restarts
/// (if asked) and ends the process.
pub fn ask(reporter: Reporter, closing: Arc<AtomicBool>) {
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
            let waited = std::time::Instant::now();
            while waited.elapsed() < UNRESPONSIVE_AFTER {
                if closing.load(Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            log::error!("the window did not respond after the device reset; ending the process");
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

/// The command that starts this executable again with the same arguments.
pub fn restart_command() -> std::io::Result<Command> {
    let exe = std::env::current_exe()?;
    let mut command = Command::new(exe);
    command.args(std::env::args_os().skip(1));
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

    use std::path::Path;
}
