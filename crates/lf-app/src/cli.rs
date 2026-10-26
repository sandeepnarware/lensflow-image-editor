//! The command line: `--version` and `--help` answer and exit before any window or
//! graphics work; anything else that looks like a flag is refused with the usage (exit 2).

use std::ffi::OsString;

/// What the command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cli {
    /// `--version` (or `-V`): print [`version_text`] and exit 0.
    Version,
    /// `--help` (or `-h`): print [`USAGE`] and exit 0.
    Help,
    /// Open the window.
    Run,
    /// A flag this build does not know: print it and the usage to stderr, exit 2.
    Unknown(String),
}

/// Reads the arguments after the program name. The first flag decides; other arguments
/// are ignored (a file manager may pass paths).
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Cli {
    for arg in args {
        let arg = arg.to_string_lossy().into_owned();
        match arg.as_str() {
            "--version" | "-V" => return Cli::Version,
            "--help" | "-h" => return Cli::Help,
            // macOS adds a process serial number when an older Finder starts the app.
            a if a.starts_with("-psn_") => {}
            a if a.starts_with('-') => return Cli::Unknown(arg),
            _ => {}
        }
    }
    Cli::Run
}

/// "lensflow-ImageEditor 0.1.0 (x86_64-pc-windows-msvc)".
pub fn version_text() -> String {
    format!(
        "lensflow-ImageEditor {} ({})",
        crate::app::VERSION,
        target()
    )
}

/// The target this build was compiled for, as a Rust target triple.
pub fn target() -> String {
    let arch = std::env::consts::ARCH;
    if cfg!(target_os = "windows") {
        let env = if cfg!(target_env = "gnu") {
            "gnu"
        } else {
            "msvc"
        };
        format!("{arch}-pc-windows-{env}")
    } else if cfg!(target_os = "macos") {
        format!("{arch}-apple-darwin")
    } else if cfg!(target_os = "linux") {
        let env = if cfg!(target_env = "musl") {
            "musl"
        } else {
            "gnu"
        };
        format!("{arch}-unknown-linux-{env}")
    } else {
        format!("{arch}-unknown-{}", std::env::consts::OS)
    }
}

/// The usage `--help` prints (and an unknown flag, to stderr).
pub const USAGE: &str = "Usage: lensflow-image-editor [--version | --help]

Starting it with no flags opens lensflow-ImageEditor.

  --version, -V   Print the version and exit.
  --help, -h      Print this help and exit.
";

/// Answers a command line other than [`Cli::Run`]; returns the exit code.
pub fn answer(cli: &Cli) -> i32 {
    attach_console();
    match cli {
        Cli::Version => {
            println!("{}", version_text());
            0
        }
        Cli::Help => {
            print!("{USAGE}");
            0
        }
        Cli::Unknown(flag) => {
            eprintln!(
                "lensflow-image-editor: unknown option {flag}
"
            );
            eprint!("{USAGE}");
            2
        }
        Cli::Run => 0,
    }
}

/// A Windows release build has no console of its own: print into the terminal it was
/// started from, when there is one. Output already redirected (a pipe or a file) stays so.
fn attach_console() {
    #[cfg(all(windows, not(debug_assertions)))]
    {
        use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
        // SAFETY: no pointers; fails harmlessly when there is no parent console.
        unsafe {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn parse_strs(args: &[&str]) -> Cli {
        parse(args.iter().map(OsString::from))
    }

    #[test]
    fn flags_are_read() {
        assert_eq!(parse_strs(&[]), Cli::Run);
        assert_eq!(parse_strs(&["--version"]), Cli::Version);
        assert_eq!(parse_strs(&["-V"]), Cli::Version);
        assert_eq!(parse_strs(&["--help"]), Cli::Help);
        assert_eq!(parse_strs(&["-h"]), Cli::Help);
        assert_eq!(parse_strs(&["--nope"]), Cli::Unknown("--nope".into()));
        // The first flag decides.
        assert_eq!(parse_strs(&["--help", "--version"]), Cli::Help);
        // macOS adds a process serial number when an old Finder starts the app.
        assert_eq!(parse_strs(&["-psn_0_12345"]), Cli::Run);
    }

    #[test]
    fn version_names_the_app_version_and_target() {
        let text = version_text();
        assert!(text.starts_with(&format!("lensflow-ImageEditor {} (", crate::app::VERSION)));
        assert!(text.ends_with(')'));
        assert!(text.contains(std::env::consts::ARCH), "{text}");
        assert!(USAGE.contains("--version") && USAGE.contains("--help"));
    }
}
