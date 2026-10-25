//! Installing a newer release: fetch the signed link, download the installer into the
//! app's cache, check its SHA-256, then hand it to the system and quit.

use lf_account::{AccountError, Client, download_link};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const NOT_VERIFIED: &str = "The download did not verify. Try again.";

/// Progress of the installer download, sent from the `Io` lane.
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

/// Downloads the latest installer for `platform` into `dest_dir` and verifies it; returns
/// its path. `report` hears the progress.
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
    let digest = fetch(&link.url, link.size, &part, &mut report);
    let verified = match digest {
        Ok(digest) => digest.eq_ignore_ascii_case(link.sha256.trim()),
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            return Err(e);
        }
    };
    if !verified {
        log::warn!("the downloaded installer's SHA-256 does not match the release");
        let _ = std::fs::remove_file(&part);
        return Err(UpdateError::Failed(NOT_VERIFIED.to_owned()));
    }
    std::fs::rename(&part, &target).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        not_finished(&e)
    })?;
    Ok(target)
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

/// Streams `url` into `part`, reporting progress about every megabyte; returns the
/// SHA-256 in hex. The link is signed, so it goes out without the account's headers.
fn fetch(
    url: &str,
    size: u64,
    part: &Path,
    report: &mut impl FnMut(DownloadProgress),
) -> Result<String, UpdateError> {
    const REPORT_EVERY: u64 = 1 << 20;
    let http = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(None::<Duration>)
        .build()
        .map_err(|e| not_finished(&e))?;
    let mut response = http
        .get(url)
        .send()
        .map_err(|e| not_finished(&e.without_url()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(not_finished(&format!(
            "The server answered {}.",
            status.as_u16()
        )));
    }
    let total = response.content_length().unwrap_or(size);
    let mut file = std::fs::File::create(part).map_err(|e| not_finished(&e))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let (mut done, mut reported) = (0u64, 0u64);
    report(DownloadProgress::Progress { done, total });
    loop {
        let n = response.read(&mut buffer).map_err(|e| not_finished(&e))?;
        if n == 0 {
            break;
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

/// Opens the downloaded installer with the system (an AppImage is made executable and
/// started directly).
pub fn launch(path: &Path) -> std::io::Result<()> {
    use std::process::Command;
    log::info!("starting the installer {}", path.display());
    #[cfg(target_os = "windows")]
    Command::new("cmd")
        .args(["/C", "start", ""])
        .arg(path)
        .spawn()?;
    #[cfg(target_os = "macos")]
    Command::new("open").arg(path).spawn()?;
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let appimage = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("appimage"));
        if appimage {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
            Command::new(path).spawn()?;
        } else {
            Command::new("xdg-open").arg(path).spawn()?;
        }
    }
    Ok(())
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
        Client::new(Instance::Custom(url), "1.0.0")
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
