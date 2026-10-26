//! When the app cannot start (no window, no graphics device it can use) or cannot open its
//! log, a release build on Windows has no console to print to: a native message box says
//! so, in plain words, and where the log is.

use rfd::{MessageButtons, MessageDialog, MessageLevel};
use std::path::Path;

/// Why the window could not open, in plain words.
pub fn reason(e: &eframe::Error) -> &'static str {
    match e {
        eframe::Error::Wgpu(_) => "no graphics device it can use was found",
        eframe::Error::AppCreation(_) => "it could not set itself up",
        _ => "its window could not be opened",
    }
}

/// "lensflow-ImageEditor could not start: {reason}. The log is at {log}."
pub fn failed_text(reason: &str, log: Option<&Path>) -> String {
    match log {
        Some(log) => format!(
            "lensflow-ImageEditor could not start: {reason}. The log is at {}.",
            log.display()
        ),
        None => format!("lensflow-ImageEditor could not start: {reason}."),
    }
}

/// The log could not be opened (`error` is `logging::init`'s sentence).
pub fn log_failed_text(error: &str) -> String {
    let stop = if error.ends_with('.') { "" } else { "." };
    format!(
        "lensflow-ImageEditor could not open its log file, so problems will not be recorded. \
         {error}{stop}"
    )
}

/// Shows `text` in a native message box (blocking) and logs it.
pub fn tell(level: MessageLevel, text: &str) {
    match level {
        MessageLevel::Error => log::error!("{text}"),
        MessageLevel::Info => log::info!("{text}"),
        _ => log::warn!("{text}"),
    }
    MessageDialog::new()
        .set_level(level)
        .set_title("lensflow-ImageEditor")
        .set_description(text)
        .set_buttons(MessageButtons::Ok)
        .show();
}

/// Whether the console is invisible (a release build on Windows opens none), so only a
/// message box can be seen.
pub const NO_CONSOLE: bool = cfg!(all(windows, not(debug_assertions)));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_failures_read_as_plain_sentences() {
        let log = Path::new("C:/data/logs/app.log");
        let e = eframe::Error::AppCreation(Box::new(std::io::Error::other("boom")));
        assert_eq!(
            failed_text(reason(&e), Some(log)),
            format!(
                "lensflow-ImageEditor could not start: it could not set itself up. The log is \
                 at {}.",
                log.display()
            )
        );
        assert_eq!(
            failed_text("its window could not be opened", None),
            "lensflow-ImageEditor could not start: its window could not be opened."
        );
        assert_eq!(
            log_failed_text("Could not open the log file x: denied"),
            "lensflow-ImageEditor could not open its log file, so problems will not be \
             recorded. Could not open the log file x: denied."
        );
    }
}
