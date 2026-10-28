//! Where the app keeps its files, and the user's preferences (`prefs.json` in the config dir).

use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

/// The preferences file's name inside the config dir.
pub const FILE_NAME: &str = "prefs.json";

/// The app's folders, from `directories::ProjectDirs`.
#[derive(Debug, Clone)]
pub struct Dirs {
    pub config: PathBuf,
    pub data: PathBuf,
    pub cache: PathBuf,
}

impl Dirs {
    /// The platform folders for `in.lensflow.lensflow-ImageEditor`; without a home
    /// directory, folders under the system temp dir.
    pub fn locate() -> Dirs {
        match directories::ProjectDirs::from("in", "lensflow", "lensflow-ImageEditor") {
            Some(dirs) => Dirs {
                config: dirs.config_dir().to_path_buf(),
                data: dirs.data_dir().to_path_buf(),
                cache: dirs.cache_dir().to_path_buf(),
            },
            None => {
                let root = std::env::temp_dir().join("lensflow-ImageEditor");
                Dirs {
                    config: root.join("config"),
                    data: root.join("data"),
                    cache: root.join("cache"),
                }
            }
        }
    }
}

/// Which LensFlow server to sign in to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceChoice {
    #[default]
    Prod,
    Dev,
}

/// How carefully the develop view decodes RAW files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DevelopQuality {
    Fast,
    #[default]
    Best,
}

/// What "Import from a card" remembers: where copies go, the rename, the backup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IngestPrefs {
    pub destination: Option<PathBuf>,
    pub rename: bool,
    pub pattern: String,
    pub backup: Option<PathBuf>,
    pub skip_duplicates: bool,
}

impl Default for IngestPrefs {
    fn default() -> IngestPrefs {
        IngestPrefs {
            destination: None,
            rename: false,
            pattern: String::new(),
            backup: None,
            skip_duplicates: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub instance: InstanceChoice,
    pub catalogue_root: Option<PathBuf>,
    pub studio_slug: Option<String>,
    pub frame_cache_bytes: u64,
    pub last_export: Option<serde_json::Value>,
    /// "Import from a card" as it was last filled in.
    pub last_ingest: Option<IngestPrefs>,
    /// The application "Edit in external editor" opens copies in.
    pub external_editor: Option<PathBuf>,
    pub develop_quality: DevelopQuality,
}

impl Default for Prefs {
    fn default() -> Prefs {
        Prefs {
            instance: InstanceChoice::default(),
            catalogue_root: None,
            studio_slug: None,
            frame_cache_bytes: 4 << 30,
            last_export: None,
            last_ingest: None,
            external_editor: None,
            develop_quality: DevelopQuality::default(),
        }
    }
}

impl Prefs {
    /// Reads `dir/prefs.json`; a missing or unreadable file gives the defaults (a corrupt
    /// one is logged).
    pub fn load(dir: &Path) -> Prefs {
        let path = dir.join(FILE_NAME);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Prefs::default(),
            Err(e) => {
                log::warn!("could not read {}: {e}; using the defaults", path.display());
                return Prefs::default();
            }
        };
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            log::warn!("{} is corrupt ({e}); using the defaults", path.display());
            Prefs::default()
        })
    }

    /// Writes `dir/prefs.json` atomically (a temporary file renamed over the old one).
    pub fn save(&self, dir: &Path) -> io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let json = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        let temp = dir.join(format!("{FILE_NAME}.tmp"));
        {
            let mut file = std::fs::File::create(&temp)?;
            io::Write::write_all(&mut file, &json)?;
            file.sync_all()?;
        }
        std::fs::rename(&temp, dir.join(FILE_NAME))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefs_round_trip_and_defaults_on_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");

        let missing = Prefs::load(&config);
        assert_eq!(missing, Prefs::default());
        assert_eq!(missing.frame_cache_bytes, 4 << 30);
        assert_eq!(missing.instance, InstanceChoice::Prod);
        assert_eq!(missing.develop_quality, DevelopQuality::Best);

        let prefs = Prefs {
            instance: InstanceChoice::Dev,
            catalogue_root: Some(PathBuf::from("photos")),
            studio_slug: Some("sharma-studio".into()),
            frame_cache_bytes: 1 << 30,
            last_export: Some(serde_json::json!({ "format": "jpeg", "quality": 90 })),
            last_ingest: Some(IngestPrefs {
                destination: Some(PathBuf::from("D:/Photos")),
                rename: true,
                pattern: "{date}_{seq}".into(),
                backup: None,
                skip_duplicates: false,
            }),
            external_editor: Some(PathBuf::from("C:/Apps/Editor.exe")),
            develop_quality: DevelopQuality::Fast,
        };
        prefs.save(&config).unwrap();
        assert_eq!(Prefs::load(&config), prefs);
        let leftovers: Vec<_> = std::fs::read_dir(&config).unwrap().collect();
        assert_eq!(
            leftovers.len(),
            1,
            "only prefs.json remains after an atomic save"
        );

        std::fs::write(config.join(FILE_NAME), b"{ not json").unwrap();
        assert_eq!(Prefs::load(&config), Prefs::default());

        // A file from an older version with fields missing keeps what it has.
        std::fs::write(config.join(FILE_NAME), br#"{ "studio_slug": "a" }"#).unwrap();
        let partial = Prefs::load(&config);
        assert_eq!(partial.studio_slug.as_deref(), Some("a"));
        assert_eq!(partial.frame_cache_bytes, 4 << 30);

        // A file from a newer version (after a downgrade): fields it added are ignored.
        let newer = br#"{ "studio_slug": "b", "gallery_sync": true, "frame_cache_bytes": 5 }"#;
        std::fs::write(config.join(FILE_NAME), newer).unwrap();
        let newer = Prefs::load(&config);
        assert_eq!(newer.studio_slug.as_deref(), Some("b"));
        assert_eq!(newer.frame_cache_bytes, 5);
    }
}
