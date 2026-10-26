//! Installing a newer release: fetch the signed link, download the installer into the
//! app's cache (`<cache>/updates`) on the `Long` lane, check its size and SHA-256, then
//! quit and hand it to the system once the app has let go of everything.
//!
//! The download streams into `<file>.part`. A part left by an interrupted download is
//! resumed with an HTTP `Range` request when the server answers `206`; any other answer
//! starts it again. More bytes than the release's size are refused (the part is deleted).
//! The file gets its own name only after its SHA-256 matches, and older files in the
//! folder are then removed.

use lf_account::{AccountError, Client, download_link};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const NOT_VERIFIED: &str = "The download did not verify. Try again.";
pub const TOO_BIG: &str = "The download was larger than the release. Try again.";

/// Progress of the installer download, sent from the `Long` lane.
#[derive(Debug)]
pub enum DownloadProgress {
    Progress { done: u64, total: u64 },
    Finished(Result<PathBuf, UpdateError>),
}

#[derive(Debug)]
pub enum UpdateError {
    /// The download link call failed (a 401 here signs the user out).
    Account(AccountError),
    /// The download itself failed; the text is ready to show.
    Failed(String),
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpdateError::Account(err) => write!(f, "Could not get the update. {err}"),
            UpdateError::Failed(message) => f.write_str(message),
        }
    }
}

/// The installer this build asks `desktop/download/{platform}` for, named as the server's
/// `DesktopRelease::PLATFORMS` names them.
pub fn platform() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if std::env::var_os("APPIMAGE").is_some() {
        "linux-appimage"
    } else {
        "linux-deb"
    }
}

/// Downloads the latest installer for `platform` into `dest_dir` (resuming a part left
/// there) and verifies it; returns its path. `report` hears the progress.
pub fn run(
    client: &Client,
    platform: &'static str,
    dest_dir: &Path,
    mut report: impl FnMut(DownloadProgress),
) -> Result<PathBuf, UpdateError> {
    let link = download_link(client, platform).map_err(UpdateError::Account)?;
    let name = file_name(&link.url)
        .unwrap_or_else(|| format!("lensflow-ImageEditor-{}-{platform}", link.version));
    std::fs::create_dir_all(dest_dir).map_err(|e| not_finished(&e))?;
    let target = dest_dir.join(&name);
    let part = dest_dir.join(format!("{name}.part"));
    log::info!(
        "downloading lensflow-ImageEditor {} ({platform})",
        link.version
    );
    // A part that is cut short stays for the next try; a refused one is already gone.
    let digest = fetch(&link.url, link.size, &part, &mut report)?;
    if !digest.eq_ignore_ascii_case(link.sha256.trim()) {
        log::warn!("the downloaded installer's SHA-256 does not match the release");
        let _ = std::fs::remove_file(&part);
        return Err(UpdateError::Failed(NOT_VERIFIED.to_owned()));
    }
    std::fs::rename(&part, &target).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        not_finished(&e)
    })?;
    remove_older(dest_dir, &target);
    Ok(target)
}

/// Removes the files in `dir` older than `keep` (earlier installers and their parts).
fn remove_older(dir: &Path, keep: &Path) {
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let Some(newest) = modified(keep) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let older = modified(&path).is_some_and(|m| m < newest);
        if path != keep && entry.file_type().is_ok_and(|t| t.is_file()) && older {
            match std::fs::remove_file(&path) {
                Ok(()) => log::info!("removed the old update file {}", path.display()),
                Err(e) => log::warn!("could not remove {}: {e}", path.display()),
            }
        }
    }
}

fn not_finished(e: &dyn std::fmt::Display) -> UpdateError {
    UpdateError::Failed(format!("The download did not finish. {e}"))
}

/// The last path segment of the signed link, when it is a plain file name.
fn file_name(url: &str) -> Option<String> {
    let url = reqwest::Url::parse(url).ok()?;
    let name = url.path_segments()?.next_back()?;
    let plain = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    (plain && !name.is_empty() && !name.starts_with('.')).then(|| name.to_owned())
}

/// Streams `url` into `part` (from the end of a part already there when the server
/// answers a `Range` request with `206`), reporting progress about every megabyte; returns
/// the whole file's SHA-256 in hex. More than `size` bytes are refused and the part
/// deleted. The link is signed, so it goes out without the account's headers.
fn fetch(
    url: &str,
    size: u64,
    part: &Path,
    report: &mut impl FnMut(DownloadProgress),
) -> Result<String, UpdateError> {
    const REPORT_EVERY: u64 = 1 << 20;
    let too_big = || {
        log::warn!("the installer download is larger than the release's {size} bytes");
        let _ = std::fs::remove_file(part);
        UpdateError::Failed(TOO_BIG.to_owned())
    };
    let http = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(None::<Duration>)
        .build()
        .map_err(|e| not_finished(&e))?;
    let have = std::fs::metadata(part).map_or(0, |m| m.len());
    let resume = have > 0 && have < size;
    if !resume && have > 0 {
        let _ = std::fs::remove_file(part);
    }
    let mut request = http.get(url);
    if resume {
        request = request.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let mut response = request.send().map_err(|e| not_finished(&e.without_url()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(not_finished(&format!(
            "The server answered {}.",
            status.as_u16()
        )));
    }
    let mut hasher = Sha256::new();
    let mut done = 0u64;
    let mut file = if resume && status == reqwest::StatusCode::PARTIAL_CONTENT {
        log::info!("resuming the download after {have} bytes");
        let mut kept = std::fs::File::open(part).map_err(|e| not_finished(&e))?;
        std::io::copy(&mut kept, &mut HashWriter(&mut hasher)).map_err(|e| not_finished(&e))?;
        done = have;
        std::fs::OpenOptions::new()
            .append(true)
            .open(part)
            .map_err(|e| not_finished(&e))?
    } else {
        std::fs::File::create(part).map_err(|e| not_finished(&e))?
    };
    if response
        .content_length()
        .is_some_and(|n| done.saturating_add(n) > size)
    {
        drop(file);
        return Err(too_big());
    }
    let total = size;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut reported = done;
    report(DownloadProgress::Progress { done, total });
    loop {
        let n = response.read(&mut buffer).map_err(|e| not_finished(&e))?;
        if n == 0 {
            break;
        }
        if done + n as u64 > size {
            drop(file);
            return Err(too_big());
        }
        hasher.update(&buffer[..n]);
        file.write_all(&buffer[..n]).map_err(|e| not_finished(&e))?;
        done += n as u64;
        if done - reported >= REPORT_EVERY {
            reported = done;
            report(DownloadProgress::Progress { done, total });
        }
    }
    file.sync_all().map_err(|e| not_finished(&e))?;
    report(DownloadProgress::Progress { done, total });
    Ok(hex::encode(hasher.finalize()))
}

/// Feeds what is written to a hasher (the part already on disk, when resuming).
struct HashWriter<'a>(&'a mut Sha256);

impl Write for HashWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.update(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The Windows command for an installer: an `.msi` through `msiexec /i <path> /passive`
/// (a progress bar, no questions; Windows asks for elevation itself), anything else run
/// directly. No `cmd.exe` is involved, so `&`, `^` or `%` in a profile path mean nothing
/// special.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn windows_installer_command(path: &Path) -> std::process::Command {
    let msi = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("msi"));
    if msi {
        let mut command = std::process::Command::new("msiexec");
        command.arg("/i").arg(path).arg("/passive");
        command
    } else {
        std::process::Command::new(path)
    }
}

/// Windows refuses to start an installer that asks for administrator rights from a plain
/// `CreateProcess` (ERROR_ELEVATION_REQUIRED).
#[cfg(target_os = "windows")]
const ERROR_ELEVATION_REQUIRED: i32 = 740;

/// What a message box says, after the window closed, when the installer could not start.
pub fn launch_failed_text(path: &Path, e: &std::io::Error) -> String {
    format!(
        "The update could not be started. {e} The installer is at {}.",
        path.display()
    )
}

/// Hands the downloaded installer to the system. Called at the very end of `on_exit`,
/// after the app has written everything and let go of its instance lock:
/// - Windows: `msiexec /i <msi> /passive`.
/// - macOS: `open <dmg>`; the disk image's window shows how to drag the app over.
/// - Linux, running from an AppImage (`$APPIMAGE`): the new AppImage replaces it (same
///   folder, a temp file then a rename, mode 755) and starts.
/// - Linux otherwise (the .deb): `xdg-open <deb>`; the system installer asks for the
///   password.
pub fn launch(path: &Path) -> std::io::Result<()> {
    use std::process::Command;
    log::info!("starting the installer {}", path.display());
    #[cfg(target_os = "windows")]
    match windows_installer_command(path).spawn() {
        Ok(_) => {}
        // Explorer opens it through the shell, which shows the elevation prompt.
        Err(e) if e.raw_os_error() == Some(ERROR_ELEVATION_REQUIRED) => {
            Command::new("explorer.exe").arg(path).spawn()?;
        }
        Err(e) => return Err(e),
    }
    #[cfg(target_os = "macos")]
    Command::new("open").arg(path).spawn()?;
    #[cfg(all(unix, not(target_os = "macos")))]
    match std::env::var_os("APPIMAGE").filter(|a| !a.is_empty()) {
        Some(appimage) => {
            let appimage = PathBuf::from(appimage);
            replace_appimage(path, &appimage)?;
            crate::device_reset::restart_command_for(Some(appimage.as_os_str()))?.spawn()?;
        }
        None => {
            Command::new("xdg-open").arg(path).spawn()?;
        }
    }
    Ok(())
}

/// Puts the AppImage `new` in place of `current` in one step: a copy beside `current`
/// (the cache may be on another file system), made executable, then renamed over it.
#[cfg(unix)]
pub fn replace_appimage(new: &Path, current: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let name = current
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp = current.with_file_name(format!(".{name}.{}.new", std::process::id()));
    let result = (|| {
        std::fs::copy(new, &temp)?;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o755))?;
        std::fs::File::open(&temp)?.sync_all()?;
        std::fs::rename(&temp, current)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use lf_account::Instance;
    use serde_json::json;

    const BYTES: &[u8] = b"pretend this is an installer";

    fn client(server: &MockServer) -> Client {
        let url = reqwest::Url::parse(&server.url("/api")).unwrap();
        Client::new(Instance::custom(url).unwrap(), "1.0.0")
            .with_token("t")
            .with_studio("frames")
    }

    fn serve(server: &MockServer, sha256: &str) {
        let url = server.url("/blobs/lensflow-ImageEditor-setup-1.3.0.exe?sig=abc");
        server.mock(|when, then| {
            when.method(GET).path("/api/desktop/download/windows");
            then.status(200).json_body(json!({"data": {
                "url": url, "sha256": sha256, "version": "1.3.0",
                "size": BYTES.len(), "expires_at": "2026-10-25T12:00:00+00:00"
            }}));
        });
        server.mock(|when, then| {
            when.method(GET)
                .path("/blobs/lensflow-ImageEditor-setup-1.3.0.exe");
            then.status(200).body(BYTES);
        });
    }

    fn files_in(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .map(|d| {
                d.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn sha_mismatch_is_rejected_and_file_removed() {
        let server = MockServer::start();
        serve(&server, &"0".repeat(64));
        let dir = tempfile::tempdir().unwrap();
        let err = run(&client(&server), "windows", dir.path(), |_| {}).unwrap_err();
        assert_eq!(err.to_string(), "The download did not verify. Try again.");
        assert!(
            files_in(dir.path()).is_empty(),
            "{:?}",
            files_in(dir.path())
        );
    }

    #[test]
    fn verified_download_lands_under_its_own_name() {
        let server = MockServer::start();
        serve(&server, &hex::encode(Sha256::digest(BYTES)));
        let dir = tempfile::tempdir().unwrap();
        let mut last = None;
        let path = run(&client(&server), "windows", dir.path(), |p| {
            if let DownloadProgress::Progress { done, total } = p {
                last = Some((done, total));
            }
        })
        .unwrap();
        assert_eq!(
            path,
            dir.path().join("lensflow-ImageEditor-setup-1.3.0.exe")
        );
        assert_eq!(std::fs::read(&path).unwrap(), BYTES);
        assert_eq!(files_in(dir.path()).len(), 1, "no .part file is left");
        let n = BYTES.len() as u64;
        assert_eq!(last, Some((n, n)));
    }

    /// The link answers with `size`; the blob serves `BYTES` whole, or from byte `from` on
    /// with `206` when asked for that range. Returns the ranged mock.
    fn serve_ranged<'a>(server: &'a MockServer, size: usize, from: usize) -> httpmock::Mock<'a> {
        let url = server.url("/blobs/lensflow-ImageEditor-1.3.0-windows-x64.msi?sig=abc");
        let sha = hex::encode(Sha256::digest(BYTES));
        server.mock(|when, then| {
            when.method(GET).path("/api/desktop/download/windows");
            then.status(200).json_body(json!({"data": {
                "url": url, "sha256": sha, "version": "1.3.0",
                "size": size, "expires_at": "2026-10-25T12:00:00+00:00"
            }}));
        });
        let ranged = server.mock(|when, then| {
            when.method(GET)
                .path("/blobs/lensflow-ImageEditor-1.3.0-windows-x64.msi")
                .header("range", format!("bytes={from}-"));
            then.status(206).body(&BYTES[from..]);
        });
        server.mock(|when, then| {
            when.method(GET)
                .path("/blobs/lensflow-ImageEditor-1.3.0-windows-x64.msi")
                .header_missing("range");
            then.status(200).body(BYTES);
        });
        ranged
    }

    const MSI: &str = "lensflow-ImageEditor-1.3.0-windows-x64.msi";

    #[test]
    fn download_resumes_from_a_part_file() {
        let server = MockServer::start();
        let ranged = serve_ranged(&server, BYTES.len(), 10);
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(format!("{MSI}.part")), &BYTES[..10]).unwrap();
        let mut first = None;
        let path = run(&client(&server), "windows", dir.path(), |p| {
            if let DownloadProgress::Progress { done, total } = p {
                first.get_or_insert((done, total));
            }
        })
        .unwrap();
        ranged.assert();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            BYTES,
            "the whole file, verified"
        );
        assert_eq!(
            first,
            Some((10, BYTES.len() as u64)),
            "progress starts where it was"
        );
        assert_eq!(files_in(dir.path()), [MSI]);
    }

    #[test]
    fn download_larger_than_link_size_is_refused() {
        let server = MockServer::start();
        serve_ranged(&server, BYTES.len() - 5, 0);
        let dir = tempfile::tempdir().unwrap();
        let err = run(&client(&server), "windows", dir.path(), |_| {}).unwrap_err();
        assert_eq!(err.to_string(), TOO_BIG);
        assert!(
            files_in(dir.path()).is_empty(),
            "{:?}",
            files_in(dir.path())
        );
    }

    #[test]
    fn a_part_longer_than_the_release_starts_again() {
        let server = MockServer::start();
        serve_ranged(&server, BYTES.len(), 0);
        let dir = tempfile::tempdir().unwrap();
        // A part file longer than the release cannot be resumed: it is dropped and the
        // download starts again.
        std::fs::write(dir.path().join(format!("{MSI}.part")), [0u8; 64]).unwrap();
        let path = run(&client(&server), "windows", dir.path(), |_| {}).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), BYTES);
    }

    #[test]
    fn old_update_files_are_cleaned() {
        let server = MockServer::start();
        serve_ranged(&server, BYTES.len(), 0);
        let dir = tempfile::tempdir().unwrap();
        let old = std::time::SystemTime::now() - Duration::from_secs(3_600);
        for name in [
            "lensflow-ImageEditor-1.2.0-windows-x64.msi",
            "lensflow-ImageEditor-1.2.5-windows-x64.msi.part",
        ] {
            let file = std::fs::File::create(dir.path().join(name)).unwrap();
            file.set_modified(old).unwrap();
        }
        std::fs::create_dir(dir.path().join("a-folder")).unwrap();
        let path = run(&client(&server), "windows", dir.path(), |_| {}).unwrap();
        assert_eq!(path, dir.path().join(MSI));
        let mut left = files_in(dir.path());
        left.sort();
        assert_eq!(left, ["a-folder", MSI], "only the new installer is kept");
    }

    #[cfg(unix)]
    #[test]
    fn an_appimage_update_replaces_the_appimage_in_one_step() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let apps = dir.path().join("Apps");
        std::fs::create_dir(&apps).unwrap();
        let current = apps.join("lensflow-ImageEditor.AppImage");
        std::fs::write(&current, b"old").unwrap();
        let new = dir
            .path()
            .join("lensflow-ImageEditor-1.3.0-linux-x86_64.AppImage");
        std::fs::write(&new, b"new").unwrap();
        replace_appimage(&new, &current).unwrap();
        assert_eq!(std::fs::read(&current).unwrap(), b"new");
        let mode = std::fs::metadata(&current).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
        assert_eq!(
            files_in(&apps),
            ["lensflow-ImageEditor.AppImage"],
            "no temp left"
        );
    }

    #[test]
    fn windows_launch_never_goes_through_cmd() {
        let exe = Path::new(r"C:\Users\R&D ^50%\AppData\lensflow-ImageEditor-setup.exe");
        let command = windows_installer_command(exe);
        assert_eq!(command.get_program(), exe.as_os_str());
        assert_eq!(command.get_args().count(), 0);

        let msi = Path::new(r"C:\Users\R&D\lensflow-ImageEditor.MSI");
        let command = windows_installer_command(msi);
        assert_eq!(command.get_program(), "msiexec");
        let args: Vec<_> = command.get_args().collect();
        let passive = std::ffi::OsStr::new("/passive");
        assert_eq!(args, [std::ffi::OsStr::new("/i"), msi.as_os_str(), passive]);
    }

    #[test]
    fn link_errors_keep_their_type() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/api/desktop/download/windows");
            then.status(401)
                .json_body(json!({"message": "Unauthenticated."}));
        });
        let dir = tempfile::tempdir().unwrap();
        let err = run(&client(&server), "windows", dir.path(), |_| {}).unwrap_err();
        assert!(matches!(
            err,
            UpdateError::Account(AccountError::Unauthorized)
        ));
    }
}
