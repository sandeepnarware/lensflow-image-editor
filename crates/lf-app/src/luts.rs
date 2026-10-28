//! The catalogue's LUT looks: `.cube` files kept in `<root>/.lensflow/luts/`, each known by
//! its file name (without `.cube`). They are read into [`lf_pipeline::lut`]'s registry when
//! the catalogue opens (before thumbnails start, so a thumbnail never misses its look), and
//! "Import .cube…" copies a new one in. A photo stores only the LUT's name
//! ([`lf_settings::lut::LutChoice`]), so a look shared between photos is one file.

use std::path::{Path, PathBuf};

use lf_settings::lut::Lut;

/// Where a catalogue keeps its LUTs.
pub fn folder(root: &Path) -> PathBuf {
    root.join(".lensflow").join("luts")
}

/// Forgets the LUTs of any catalogue opened before and reads those of `root`; returns how
/// many were loaded. A file that cannot be read is logged and left out.
pub fn load_all(root: &Path) -> usize {
    lf_pipeline::lut::clear();
    let Ok(entries) = std::fs::read_dir(folder(root)) else {
        return 0;
    };
    let mut loaded = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("cube"))
        {
            continue;
        }
        let Some(name) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
            continue;
        };
        match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|text| Lut::parse_cube(&text).map_err(|e| e.to_string()))
        {
            Ok(lut) => {
                lf_pipeline::lut::register(&name, lut);
                loaded += 1;
            }
            Err(e) => log::warn!("the LUT {} could not be read: {e}", path.display()),
        }
    }
    log::info!("{loaded} LUT looks loaded");
    loaded
}

/// Copies the `.cube` file at `file` into the catalogue's LUT folder (replacing one of the
/// same name) and loads it; returns its name. Nothing is copied when it cannot be read.
pub fn import(root: &Path, file: &Path) -> Result<String, String> {
    let text =
        std::fs::read_to_string(file).map_err(|e| format!("The file could not be read ({e})."))?;
    let lut = Lut::parse_cube(&text).map_err(|e| e.to_string())?;
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "LUT".to_owned());
    let name = lf_catalog::export::safe_stem(stem.trim());
    let dir = folder(root);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("The LUT folder could not be made ({e})."))?;
    let to = dir.join(format!("{name}.cube"));
    let tmp = dir.join(format!(".{name}.cube.{}.tmp", std::process::id()));
    std::fs::write(&tmp, text.as_bytes())
        .and_then(|()| std::fs::rename(&tmp, &to))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("The LUT could not be saved ({e}).")
        })?;
    lf_pipeline::lut::register(&name, lut);
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube(size: usize) -> String {
        let mut text = format!("LUT_3D_SIZE {size}\n");
        for p in Lut::identity(size).data {
            text.push_str(&format!("{} {} {}\n", p[0], p[1], p[2]));
        }
        text
    }

    #[test]
    fn an_imported_lut_is_kept_and_loaded_again_with_the_catalogue() {
        let root = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let file = src.path().join("Film Warm.cube");
        std::fs::write(&file, cube(3)).unwrap();
        assert_eq!(import(root.path(), &file).unwrap(), "Film Warm");
        assert!(folder(root.path()).join("Film Warm.cube").exists());
        assert!(lf_pipeline::lut::get("Film Warm").is_some());

        let bad = src.path().join("bad.cube");
        std::fs::write(&bad, "LUT_1D_SIZE 4\n").unwrap();
        assert!(import(root.path(), &bad).unwrap_err().contains("1D"));
        assert!(!folder(root.path()).join("bad.cube").exists());

        // Opening the catalogue again reads it back (a bad file in the folder is skipped).
        std::fs::write(folder(root.path()).join("broken.cube"), "nonsense").unwrap();
        assert_eq!(load_all(root.path()), 1);
        assert!(lf_pipeline::lut::get("Film Warm").is_some());
    }
}
