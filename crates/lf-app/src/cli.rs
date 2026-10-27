//! The command line: `--version`, `--help` and `grade` answer and exit before any window or
//! graphics work; anything else that looks like a flag is refused with the usage (exit 2).

use crate::grade_cli;
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
    /// `grade …`: the console's grade (see [`grade_cli`]).
    Grade(Box<grade_cli::Args>),
    /// `grade --help`: print [`grade_cli::USAGE`] and exit 0.
    GradeHelp,
    /// An option `grade` does not know: print it and [`grade_cli::USAGE`], exit 2.
    GradeBad(String),
    /// A flag this build does not know: print it and the usage to stderr, exit 2.
    Unknown(String),
}

/// Reads the arguments after the program name. A leading `grade` hands everything after it to
/// [`grade_cli::parse`]; otherwise the first flag decides, and other arguments are ignored (a
/// file manager may pass paths).
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Cli {
    let args: Vec<OsString> = args.into_iter().collect();
    if args
        .first()
        .is_some_and(|a| a.to_string_lossy().eq_ignore_ascii_case("grade"))
    {
        let rest = args
            .into_iter()
            .skip(1)
            .map(|a| a.to_string_lossy().into_owned());
        return match grade_cli::parse(rest) {
            grade_cli::Parsed::Run(args) => Cli::Grade(args),
            grade_cli::Parsed::Help => Cli::GradeHelp,
            grade_cli::Parsed::Bad(message) => Cli::GradeBad(message),
        };
    }
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

/// "lensflow-ImageEditor 0.1.1 (x86_64-pc-windows-msvc)".
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
pub const USAGE: &str = "Usage: lensflow-image-editor [--version | --help | grade …]

Starting it with no flags opens lensflow-ImageEditor.

  --version, -V   Print the version and exit.
  --help, -h      Print this help and exit.
  grade …         Grade a folder of photos to match a reference picture, in this window.
                  `grade --help` lists its options.
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
        Cli::Grade(args) => grade_cli::run(args),
        Cli::GradeHelp => {
            print!("{}", grade_cli::USAGE);
            0
        }
        Cli::GradeBad(message) => {
            eprintln!("lensflow-image-editor grade: {message}\n");
            eprint!("{}", grade_cli::USAGE);
            2
        }
        Cli::Unknown(flag) => {
            eprintln!("lensflow-image-editor: unknown option {flag}\n");
            eprint!("{USAGE}");
            2
        }
        Cli::Run => 0,
    }
}

/// A Windows release build has no console of its own: print into the terminal it was started
/// from, when there is one. Output already redirected (a pipe or a file) stays so.
///
/// Returns whether this process has a console to print into afterwards. A build started from
/// Explorer has none, and no console window is made here: `--version` from the Start menu must
/// not flash a window. `grade` asks for one through [`ensure_console`].
pub(crate) fn attach_console() -> bool {
    #[cfg(all(windows, not(debug_assertions)))]
    {
        use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
        // SAFETY: no pointers; fails harmlessly when there is no parent console.
        return unsafe { AttachConsole(ATTACH_PARENT_PROCESS) != 0 };
    }
    #[cfg(not(all(windows, not(debug_assertions))))]
    {
        // A console build already has its terminal (and stdout may be redirected).
        true
    }
}

/// A console for an interactive command: the one this process was started from, or a new
/// window when it has none (a release build started from Explorer).
///
/// Returns whether the window was made here, so the caller can keep it open after the work.
pub(crate) fn ensure_console() -> bool {
    #[cfg(all(windows, not(debug_assertions)))]
    {
        use windows_sys::Win32::System::Console::{
            ATTACH_PARENT_PROCESS, AllocConsole, AttachConsole, GetConsoleWindow,
        };
        // SAFETY: no pointers, and the handles are checked before use.
        unsafe {
            if !GetConsoleWindow().is_null() {
                bind_stdio();
                return false;
            }
            if AttachConsole(ATTACH_PARENT_PROCESS) != 0 {
                bind_stdio();
                return false;
            }
            if AllocConsole() == 0 {
                return false;
            }
            bind_stdio();
            true
        }
    }
    #[cfg(not(all(windows, not(debug_assertions))))]
    {
        false
    }
}

/// Points the process's standard handles at its console when they were never set (a GUI build
/// has none at start, so `println!` would go nowhere even after attaching). Handles a shell did
/// set — a redirected file or a pipe — are left exactly as they are.
#[cfg(all(windows, not(debug_assertions)))]
unsafe fn bind_stdio() {
    use std::ptr;
    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
    };

    for (std, name, access) in [
        (STD_INPUT_HANDLE, "CONIN$\0", GENERIC_READ),
        (STD_OUTPUT_HANDLE, "CONOUT$\0", GENERIC_READ | GENERIC_WRITE),
        (STD_ERROR_HANDLE, "CONOUT$\0", GENERIC_READ | GENERIC_WRITE),
    ] {
        // SAFETY: the name is nul-terminated and the security attributes are null.
        unsafe {
            let current = GetStdHandle(std);
            if !current.is_null() && current != INVALID_HANDLE_VALUE {
                continue;
            }
            let wide: Vec<u16> = name.encode_utf16().collect();
            let handle = CreateFileW(
                wide.as_ptr(),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                ptr::null(),
                OPEN_EXISTING,
                0,
                ptr::null_mut(),
            );
            if handle != INVALID_HANDLE_VALUE {
                SetStdHandle(std, handle);
            }
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
        // A bare path is a file manager's, not the grade's.
        assert_eq!(parse_strs(&["D:/shots"]), Cli::Run);
    }

    #[test]
    fn grade_has_its_own_command_line() {
        let Cli::Grade(args) = parse_strs(&["grade"]) else {
            panic!("grade is not the grade command");
        };
        assert_eq!(*args, grade_cli::Args::default());
        assert_eq!(parse_strs(&["grade", "--help"]), Cli::GradeHelp);
        assert!(matches!(
            parse_strs(&["grade", "--strength", "50", "-r", "a", "-f", "b", "--yes"]),
            Cli::Grade(a) if a.strength == 50 && a.yes
        ));
        assert!(matches!(
            parse_strs(&["grade", "--nope"]),
            Cli::GradeBad(m) if m.contains("--nope")
        ));
        // Case does not matter, and `--version` after `grade` is an unknown grade option.
        assert!(matches!(parse_strs(&["GRADE"]), Cli::Grade(_)));
        assert!(matches!(
            parse_strs(&["grade", "--version"]),
            Cli::GradeBad(_)
        ));
    }

    #[test]
    fn version_names_the_app_version_and_target() {
        let text = version_text();
        assert!(text.starts_with(&format!("lensflow-ImageEditor {} (", crate::app::VERSION)));
        assert!(text.ends_with(')'));
        assert!(text.contains(std::env::consts::ARCH), "{text}");
        assert!(USAGE.contains("--version") && USAGE.contains("--help"));
        assert!(USAGE.contains("grade"), "the usage names the grade command");
        // A grade help line exists for every flag docs/app.md promises.
        for flag in [
            "--reference",
            "--folder",
            "--out",
            "--strength",
            "--exposure",
            "--recursive",
            "--yes",
            "--dry-run",
        ] {
            assert!(grade_cli::USAGE.contains(flag), "{flag} is not documented");
        }
    }
}
