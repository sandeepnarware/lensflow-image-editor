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
    failures().clear();
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
    write_cube(root, &name, text.as_bytes())?;
    lf_pipeline::lut::register(&name, lut);
    Ok(name)
}

/// Writes `<root>/.lensflow/luts/<name>.cube` atomically (a temporary file, then a rename).
fn write_cube(root: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    let dir = folder(root);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("The LUT folder could not be made ({e})."))?;
    let to = dir.join(format!("{name}.cube"));
    let tmp = dir.join(format!(".{name}.cube.{}.tmp", std::process::id()));
    std::fs::write(&tmp, bytes)
        .and_then(|()| std::fs::rename(&tmp, &to))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("The LUT could not be saved ({e}).")
        })
}

/// Makes the colour table a preset carries (`desktop.lut.table`) available in this catalogue:
/// writes `lr-<hash>.cube` if it is not there yet, registers it, and returns its name (none
/// for a preset with no table). Called before a preset is applied, so the photo's stored
/// name ([`lf_settings::lut::name_embedded`]) renders. An error is ready to show; the preset
/// can still be applied (the photo renders without the table until it is fixed).
pub fn ensure_embedded(root: &Path, preset: &serde_json::Value) -> Result<Option<String>, String> {
    let Some(desktop_lut) = preset.get("desktop").and_then(|d| d.get("lut")) else {
        return Ok(None);
    };
    if desktop_lut.get("table").is_none() {
        return Ok(None);
    }
    // Hover calls this every frame: the common case is a table already registered, answered
    // from its hash alone with no decoding, and a failed write is remembered, not retried.
    let Some(hash) = lf_settings::lut::well_formed_hash(desktop_lut) else {
        return Err(CORRUPT_TABLE.to_owned());
    };
    let name = lf_settings::lut::embedded_name(hash);
    if lf_pipeline::lut::get(&name).is_some() {
        return Ok(Some(name));
    }
    let key = (root.to_path_buf(), name.clone());
    if let Some(e) = failures().get(&key) {
        return Err(e.clone());
    }
    let Some(mut lut) = lf_settings::lut::decode_table(desktop_lut) else {
        failures().insert(key, CORRUPT_TABLE.to_owned());
        return Err(CORRUPT_TABLE.to_owned());
    };
    if !folder(root).join(format!("{name}.cube")).exists() {
        lut.title = Some(name.clone());
        if let Err(e) = write_cube(root, &name, lut.to_cube().as_bytes()) {
            failures().insert(key, e.clone());
            return Err(e);
        }
    }
    lf_pipeline::lut::register(&name, lut);
    Ok(Some(name))
}

/// Writes the colour table `preset` carries to the catalogue's LUT folder when its file is
/// missing, *whether or not it is registered*, and registers it. For a preset just made by
/// "Match a Lightroom example…": the fit registered its table to render it before any file
/// existed, and [`ensure_embedded`] trusts a registered table to have its file (it answers
/// hovers from the registry alone). A preset without a table does nothing.
pub fn write_embedded(root: &Path, preset: &serde_json::Value) -> Result<(), String> {
    let Some(desktop_lut) = preset
        .pointer("/desktop/lut")
        .filter(|l| l.get("table").is_some())
    else {
        return Ok(());
    };
    let (Some(hash), Some(mut lut)) = (
        lf_settings::lut::well_formed_hash(desktop_lut),
        lf_settings::lut::decode_table(desktop_lut),
    ) else {
        return Err(CORRUPT_TABLE.to_owned());
    };
    let name = lf_settings::lut::embedded_name(hash);
    if !folder(root).join(format!("{name}.cube")).exists() {
        lut.title = Some(name.clone());
        write_cube(root, &name, lut.to_cube().as_bytes())?;
    }
    failures().remove(&(root.to_path_buf(), name.clone()));
    lf_pipeline::lut::register(&name, lut);
    Ok(())
}

/// The registered names the LUT panel offers: an embedded table (`lr-`) only when its
/// `.cube` file is in `root`'s LUT folder, or the open photo already uses it (`current`).
/// One without a file (a fit's leftover) would be lost with the next catalogue open.
pub fn offered(names: Vec<String>, root: Option<&Path>, current: Option<&str>) -> Vec<String> {
    names
        .into_iter()
        .filter(|n| {
            !n.starts_with("lr-")
                || current == Some(n.as_str())
                || root.is_some_and(|r| folder(r).join(format!("{n}.cube")).exists())
        })
        .collect()
}

/// The registry is process-wide and [`load_all`] clears it, so tests that register or read
/// tables take turns through this.
#[cfg(test)]
pub(crate) static REGISTRY_TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());

type Failures = std::collections::HashMap<(PathBuf, String), String>;

/// Tables that could not be made available, so a hover does not retry every frame. Cleared
/// whenever a catalogue's LUTs are loaded (opening it again is a fresh try).
fn failures() -> std::sync::MutexGuard<'static, Failures> {
    static F: std::sync::OnceLock<std::sync::Mutex<Failures>> = std::sync::OnceLock::new();
    F.get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// The toast when a preset with a colour table is applied with no catalogue open.
pub const NO_CATALOGUE: &str =
    "This preset's colour table cannot be stored without a catalogue, so it is applied without it.";

/// Whether `preset` carries a colour table.
pub fn carries_table(preset: &serde_json::Value) -> bool {
    preset.pointer("/desktop/lut/table").is_some()
}

/// The toast when a preset's colour table cannot be read.
pub const CORRUPT_TABLE: &str =
    "This preset's colour table is damaged, so it is applied without it.";

#[cfg(test)]
mod tests {
    use super::*;

    use super::REGISTRY_TURN as REGISTRY;

    fn cube(size: usize) -> String {
        let mut text = format!("LUT_3D_SIZE {size}\n");
        for p in Lut::identity(size).data {
            text.push_str(&format!("{} {} {}\n", p[0], p[1], p[2]));
        }
        text
    }

    #[test]
    fn the_panel_hides_embedded_tables_without_a_file() {
        let root = tempfile::tempdir().unwrap();
        write_cube(root.path(), "lr-saved", b"x").unwrap();
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let all = names(&["Film", "lr-saved", "lr-fileless", "lr-used"]);
        assert_eq!(
            offered(all.clone(), Some(root.path()), Some("lr-used")),
            names(&["Film", "lr-saved", "lr-used"])
        );
        assert_eq!(offered(all, None, None), names(&["Film"]));
    }

    #[test]
    fn an_imported_lut_is_kept_and_loaded_again_with_the_catalogue() {
        let _turn = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
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

    #[test]
    fn ensure_embedded_writes_once_and_registers() {
        let _turn = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        let root = tempfile::tempdir().unwrap();
        let mut lut = Lut::identity(5);
        lut.data[7] = [0.5, 0.25, 0.125];
        let preset = serde_json::json!({"desktop": {"lut": lf_settings::lut::encode_table(&lut)}});
        let name = ensure_embedded(root.path(), &preset).unwrap().unwrap();
        assert!(name.starts_with("lr-"));
        let file = folder(root.path()).join(format!("{name}.cube"));
        assert!(file.exists());
        assert!(lf_pipeline::lut::get(&name).is_some());
        let back = Lut::parse_cube(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(back.size, 5);

        // A second call leaves the file alone.
        std::fs::write(&file, "marker").unwrap();
        assert_eq!(
            ensure_embedded(root.path(), &preset).unwrap().unwrap(),
            name
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "marker");

        assert_eq!(
            ensure_embedded(root.path(), &serde_json::json!({"v": 2})),
            Ok(None)
        );
        let plain = serde_json::json!({"desktop": {"lut": {"name": "Film"}}});
        assert_eq!(ensure_embedded(root.path(), &plain), Ok(None));
        let mut bad = preset.clone();
        bad["desktop"]["lut"]["table"] = "AAAA".into();
        bad["desktop"]["lut"]["hash"] = "0123456789abcdef".into();
        assert_eq!(
            ensure_embedded(root.path(), &bad),
            Err(CORRUPT_TABLE.to_owned())
        );
    }

    #[test]
    fn a_registered_table_is_not_decoded_again_and_a_failed_write_is_not_retried() {
        let _turn = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        let root = tempfile::tempdir().unwrap();
        let mut preset = serde_json::json!({"desktop": {"lut": lf_settings::lut::encode_table(&Lut::identity(4))}});
        let name = ensure_embedded(root.path(), &preset).unwrap().unwrap();
        // A damaged body with the registered hash still answers: nothing was decoded.
        preset["desktop"]["lut"]["table"] = "AAAA".into();
        assert_eq!(
            ensure_embedded(root.path(), &preset).unwrap().unwrap(),
            name
        );

        // A write that fails (the LUT folder's place is a file) is not tried again.
        let blocked = tempfile::tempdir().unwrap();
        std::fs::create_dir(blocked.path().join(".lensflow")).unwrap();
        std::fs::write(folder(blocked.path()), "in the way").unwrap();
        let other = serde_json::json!({"desktop": {"lut": lf_settings::lut::encode_table(&Lut::identity(3))}});
        let first = ensure_embedded(blocked.path(), &other).unwrap_err();
        std::fs::remove_file(folder(blocked.path())).unwrap();
        assert_eq!(ensure_embedded(blocked.path(), &other).unwrap_err(), first);
        assert!(!folder(blocked.path()).exists());
    }

    #[test]
    fn a_registered_table_without_its_file_gets_one() {
        let _turn = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
        let root = tempfile::tempdir().unwrap();
        let mut lut = Lut::identity(3);
        lut.data[4] = [0.5, 0.5, 0.25];
        let encoded = lf_settings::lut::encode_table(&lut);
        let name =
            lf_settings::lut::embedded_name(lf_settings::lut::well_formed_hash(&encoded).unwrap());
        // As the example fit leaves it: registered, no file.
        lf_pipeline::lut::register(&name, lut);
        let preset = serde_json::json!({"desktop": {"lut": encoded}});
        let file = folder(root.path()).join(format!("{name}.cube"));
        assert_eq!(
            ensure_embedded(root.path(), &preset),
            Ok(Some(name.clone()))
        );
        assert!(!file.exists(), "ensure_embedded trusts the registry");
        write_embedded(root.path(), &preset).unwrap();
        assert!(file.exists());
        assert_eq!(
            write_embedded(root.path(), &serde_json::json!({"v": 2})),
            Ok(())
        );
    }
}
