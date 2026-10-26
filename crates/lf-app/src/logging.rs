//! Logging to `<data dir>/logs/app.log` (rotated at 5 MB, three old files kept), and to
//! stderr in debug builds. `RUST_LOG` overrides the default `info` level.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The log file rotates when it passes this size.
pub const MAX_BYTES: u64 = 5 << 20;
/// How many rotated files (`app.log.1` to `app.log.3`) are kept.
pub const KEEP: usize = 3;

/// The log folder inside the data dir.
pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("logs")
}

/// Starts logging and returns the log file's path. Logs the version and OS; panics are
/// logged too (then reported as before).
pub fn init(data_dir: &Path) -> Result<PathBuf, String> {
    let dir = dir(data_dir);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Could not create the log folder {}: {e}", dir.display()))?;
    let path = dir.join("app.log");
    let file = RotatingFile::open(path.clone(), MAX_BYTES, cfg!(debug_assertions))
        .map_err(|e| format!("Could not open the log file {}: {e}", path.display()))?;
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .target(env_logger::Target::Pipe(Box::new(file)))
        .write_style(env_logger::WriteStyle::Never)
        .try_init()
        .map_err(|e| e.to_string())?;

    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("panic: {info}");
        previous(info);
    }));

    log::info!(
        "lensflow-ImageEditor {} on {} {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    Ok(path)
}

/// Appends to `path`, renaming it to `path.1` (and older files up to `path.KEEP`) once it
/// grows past `max_bytes`; with `echo`, every line also goes to stderr.
struct RotatingFile {
    path: PathBuf,
    max_bytes: u64,
    /// `None` after a rotation until the next write reopens `path` (Windows will not rename
    /// an open file).
    file: Option<File>,
    written: u64,
    echo: bool,
}

impl RotatingFile {
    fn open(path: PathBuf, max_bytes: u64, echo: bool) -> io::Result<RotatingFile> {
        let file = open_append(&path)?;
        let written = file.metadata()?.len();
        Ok(RotatingFile {
            path,
            max_bytes,
            file: Some(file),
            written,
            echo,
        })
    }

    fn rotated(&self, n: usize) -> PathBuf {
        let mut name = self.path.clone().into_os_string();
        name.push(format!(".{n}"));
        PathBuf::from(name)
    }

    /// Shifts the files; the next write reopens `path`. If the shift fails (say, a viewer
    /// holds `app.log.1` open), logging carries on in the current file and the next attempt
    /// comes after another `max_bytes`.
    fn rotate(&mut self) {
        self.file = None;
        if let Err(e) = self.shift()
            && self.echo
        {
            eprintln!("could not rotate {}: {e}", self.path.display());
        }
        self.written = 0;
    }

    /// `path.N` becomes `path.N+1` (the oldest is replaced), then `path` becomes `path.1`.
    fn shift(&self) -> io::Result<()> {
        for n in (1..KEEP).rev() {
            let from = self.rotated(n);
            if from.exists() {
                std::fs::rename(&from, self.rotated(n + 1))?;
            }
        }
        std::fs::rename(&self.path, self.rotated(1))
    }
}

fn open_append(path: &Path) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

impl Write for RotatingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.echo {
            let _ = io::stderr().write_all(buf);
        }
        if self.written >= self.max_bytes {
            self.rotate();
        }
        let file = match &mut self.file {
            Some(file) => file,
            None => self.file.insert(open_append(&self.path)?),
        };
        file.write_all(buf)?;
        self.written += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        match &mut self.file {
            Some(file) => file.flush(),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_lines(log: &mut RotatingFile, count: usize) {
        for n in 1..=count {
            log.write_all(format!("line {n} ----\n").as_bytes())
                .unwrap();
        }
        log.flush().unwrap();
    }

    #[test]
    fn log_file_rotates_and_keeps_three_old_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = RotatingFile::open(dir.path().join("app.log"), 10, false).unwrap();
        write_lines(&mut log, 5);
        let read = |name: &str| std::fs::read_to_string(dir.path().join(name)).unwrap();
        assert_eq!(read("app.log"), "line 5 ----\n");
        assert_eq!(read("app.log.1"), "line 4 ----\n");
        assert_eq!(read("app.log.2"), "line 3 ----\n");
        assert_eq!(read("app.log.3"), "line 2 ----\n");
        assert!(
            !dir.path().join("app.log.4").exists(),
            "line 1's file is dropped"
        );
    }

    #[test]
    fn a_failed_rotation_keeps_appending_to_the_current_file() {
        let dir = tempfile::tempdir().unwrap();
        // Non-empty folders where the rotated files go: renaming onto them fails.
        for n in 1..=KEEP {
            let blocker = dir.path().join(format!("app.log.{n}"));
            std::fs::create_dir(&blocker).unwrap();
            std::fs::write(blocker.join("keep"), b"x").unwrap();
        }
        let mut log = RotatingFile::open(dir.path().join("app.log"), 10, false).unwrap();
        write_lines(&mut log, 4);
        let kept = std::fs::read_to_string(dir.path().join("app.log")).unwrap();
        assert_eq!(kept, "line 1 ----\nline 2 ----\nline 3 ----\nline 4 ----\n");
    }
}
