//! Small helpers that differ by operating system.

use std::path::Path;

/// Shows `path` in the system's file manager (selected where the system can).
pub fn show_in_folder(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        std::process::Command::new("explorer")
            .raw_arg(format!("/select,\"{}\"", path.display()))
            .spawn()
            .map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn()
            .map(|_| ())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(path.parent().unwrap_or(path))
            .spawn()
            .map(|_| ())
    }
}

/// Opens `file` in the application at `app` (an `.exe`, an `.app` bundle on macOS, or a
/// program on Linux).
pub fn open_with(app: &Path, file: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        if app.extension().is_some_and(|e| e == "app") {
            return std::process::Command::new("open")
                .arg("-a")
                .arg(app)
                .arg(file)
                .spawn()
                .map(|_| ());
        }
    }
    std::process::Command::new(app)
        .arg(file)
        .spawn()
        .map(|_| ())
}

/// Opens `file` with the system's default application for it.
pub fn open_file(file: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        std::process::Command::new("explorer")
            .arg(file)
            .spawn()
            .map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(file)
            .spawn()
            .map(|_| ())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(file)
            .spawn()
            .map(|_| ())
    }
}
