//! The catalogue's camera profiles: DNG `.dcp` files kept in `<root>/.lensflow/profiles/`,
//! each known by its file name (without `.dcp`). They are read into [`lf_raw::dcp`]'s
//! registry when the catalogue opens (before thumbnails start, so a thumbnail never misses
//! its profile), and "Import .dcp…" copies a new one in. A photo stores only the profile's
//! name (`desktop.develop.dcp`), so a profile shared between photos is one file. No profile
//! ships with the app.

use std::path::{Path, PathBuf};

use lf_raw::dcp::Dcp;

/// Where a catalogue keeps its camera profiles.
pub fn folder(root: &Path) -> PathBuf {
    root.join(".lensflow").join("profiles")
}

/// Forgets the profiles of any catalogue opened before and reads those of `root`; returns
/// how many were loaded. A file that cannot be read is logged and left out.
pub fn load_all(root: &Path) -> usize {
    lf_raw::dcp::clear();
    let Ok(entries) = std::fs::read_dir(folder(root)) else {
        return 0;
    };
    let mut loaded = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("dcp"))
        {
            continue;
        }
        let Some(name) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
            continue;
        };
        match std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| Dcp::parse(&bytes).map_err(|e| e.to_string()))
        {
            Ok(dcp) => {
                lf_raw::dcp::register(&name, dcp);
                loaded += 1;
            }
            Err(e) => log::warn!(
                "the camera profile {} could not be read: {e}",
                path.display()
            ),
        }
    }
    log::info!("{loaded} camera profiles loaded");
    loaded
}

/// Copies the `.dcp` file at `file` into the catalogue's profile folder (replacing one of
/// the same name) and loads it; returns its name. Nothing is copied when it cannot be read.
pub fn import(root: &Path, file: &Path) -> Result<String, String> {
    let bytes = std::fs::read(file).map_err(|e| format!("The file could not be read ({e})."))?;
    let dcp = Dcp::parse(&bytes).map_err(|e| e.to_string())?;
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Profile".to_owned());
    let name = lf_catalog::export::safe_stem(stem.trim());
    let dir = folder(root);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("The profile folder could not be made ({e})."))?;
    let to = dir.join(format!("{name}.dcp"));
    let tmp = dir.join(format!(".{name}.dcp.{}.tmp", std::process::id()));
    std::fs::write(&tmp, &bytes)
        .and_then(|()| std::fs::rename(&tmp, &to))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("The profile could not be saved ({e}).")
        })?;
    lf_raw::dcp::register(&name, dcp);
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lf_raw::dcp::testing::{Tag, ascii, srational, write};

    fn profile() -> Vec<u8> {
        write(vec![
            Tag(50708, 2, 9, ascii("Test Cam")),
            Tag(
                50964,
                10,
                9,
                srational(&[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]),
            ),
        ])
    }

    #[test]
    fn an_imported_profile_is_kept_and_loaded_again_with_the_catalogue() {
        let root = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let file = src.path().join("Test Cam Standard.dcp");
        std::fs::write(&file, profile()).unwrap();
        assert_eq!(import(root.path(), &file).unwrap(), "Test Cam Standard");
        assert!(folder(root.path()).join("Test Cam Standard.dcp").exists());
        let id = lf_settings::dcp_id("Test Cam Standard");
        assert_eq!(lf_raw::dcp::get(id).unwrap().camera, "Test Cam");

        let bad = src.path().join("bad.dcp");
        std::fs::write(&bad, b"not a profile").unwrap();
        assert!(import(root.path(), &bad).is_err());
        assert!(!folder(root.path()).join("bad.dcp").exists());

        // Opening the catalogue again reads it back (a bad file in the folder is skipped).
        std::fs::write(folder(root.path()).join("broken.dcp"), b"IIRC").unwrap();
        assert_eq!(load_all(root.path()), 1);
        assert!(lf_raw::dcp::get(id).is_some());
    }
}
