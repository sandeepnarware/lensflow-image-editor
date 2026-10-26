//! The built binary's command line: `--version` and `--help` print and exit before any
//! window or graphics work (no window opens in a test), and an unknown flag exits 2.

use std::process::{Command, Output};
use std::time::{Duration, Instant};

/// Runs the app with `args`; fails if it has not exited within 30 s (a window opened).
fn run(args: &[&str]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lensflow-image-editor"))
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the binary starts");
    let end = Instant::now() + Duration::from_secs(30);
    loop {
        if child
            .try_wait()
            .expect("the binary can be waited for")
            .is_some()
        {
            return child.wait_with_output().expect("its output");
        }
        if Instant::now() > end {
            let _ = child.kill();
            panic!("lensflow-image-editor {args:?} did not exit (it must not open a window)");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn version_flag_prints_and_exits_before_any_window() {
    let out = run(&["--version"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.starts_with("lensflow-ImageEditor 0."), "{stdout:?}");
    assert!(
        stdout.trim_end().ends_with(')'),
        "the target triple follows in brackets: {stdout:?}"
    );
}

#[test]
fn help_flag_prints_usage_and_exits_0() {
    let out = run(&["--help"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0));
    assert!(
        stdout.contains("--version") && stdout.contains("--help"),
        "{stdout:?}"
    );
}

#[test]
fn unknown_flag_exits_2() {
    let out = run(&["--frobnicate"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr.contains("--frobnicate"), "{stderr:?}");
    assert!(stderr.contains("--version"), "usage follows: {stderr:?}");
    assert!(out.stdout.is_empty());
}
