//! Opening a photo for Develop, on the `Open` lane: reconcile its sidecar with the catalogue,
//! decode it (through the frame cache for RAW), and build its `Renderer`, so the UI thread
//! only ever receives a renderer ready to draw (it never uploads pixels).

use crate::jobs::Msg;
use crate::prefs::DevelopQuality;
use lf_catalog::Catalog;
use lf_catalog::edits::{self, OnOpen};
use lf_pipeline::{Gpu, Renderer};
use lf_raw::RawError;
use lf_raw::cache::{CacheKey, FrameCache};
use lf_raw::demosaic::Quality;
use lf_raw::exif::ExifSummary;
use lf_raw::open::{Kind, Opened, Picture};
use lf_settings::{DevelopSettings, EditSettings};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// The longest edge of Develop's working copy: a 6k copy keeps a 60 MP RAW under 300 MB of
/// half floats.
pub const WORKING_EDGE: u32 = 6144;

/// An open answer that a newer request overtook before it started (dropped by the UI).
pub const SUPERSEDED: &str = "superseded by a newer request";

/// The working copy's long edge on this device.
pub fn max_edge(gpu: &Gpu) -> u32 {
    gpu.max_edge().min(WORKING_EDGE)
}

/// The demosaic quality for the preference.
pub fn quality(pref: DevelopQuality) -> Quality {
    match pref {
        DevelopQuality::Fast => Quality::Fast,
        DevelopQuality::Best => Quality::Best,
    }
}

/// Opens `path` as [`lf_raw::open::open`] does, through `cache` for RAW files: a cached
/// developed frame skips the decode and demosaic; a fresh one is stored. 8-bit files bypass
/// the cache (decoding them is quick).
pub fn open_cached(
    path: &Path,
    quality: Quality,
    settings: &DevelopSettings,
    max_edge: u32,
    cache: &FrameCache,
) -> Result<Opened, RawError> {
    if lf_raw::open::sniff(path)? != Kind::Raw {
        return lf_raw::open::open(path, quality, settings, max_edge);
    }
    let key = CacheKey::for_file(path, settings, quality, max_edge)?;
    if let Some(developed) = cache.get(&key) {
        // The EXIF facts come from the headers alone (no pixels decoded).
        let (_, exif, _) = lf_raw::open::summary(path)?;
        return Ok(Opened {
            full_width: developed.full_width,
            full_height: developed.full_height,
            reduced: developed.reduced,
            picture: Picture::Float(developed),
            kind: Kind::Raw,
            exif,
            jpeg_exif_bytes: None,
        });
    }
    let opened = lf_raw::open::open(path, quality, settings, max_edge)?;
    if let Picture::Float(developed) = &opened.picture {
        cache.put(&key, developed);
    }
    Ok(opened)
}

/// A photo ready for Develop: its renderer (the pixels are already on the device) and what
/// the view shows about it. Holds no pixels.
pub struct OpenedPhoto {
    pub renderer: Renderer,
    pub kind: Kind,
    pub exif: ExifSummary,
    /// The file's size (after orientation, before any reduction).
    pub full_width: u32,
    pub full_height: u32,
    /// The working copy was reduced to fit [`WORKING_EDGE`] (or the device's limit).
    pub reduced: bool,
    /// The photo, small, for the eyedropper and the auto buttons ([`super::auto`]).
    pub sample: super::auto::Sample,
    /// The RAW's as-shot colour temperature (kelvin), when its headers say.
    pub as_shot_kelvin: Option<f64>,
    /// The develop settings the RAW was decoded with.
    pub develop: DevelopSettings,
    /// The photo's edits from the catalogue (after the sidecar was reconciled).
    pub settings: EditSettings,
    /// Why the sidecar beside the photo could not be read, when it could not.
    pub sidecar_error: Option<String>,
    pub file_name: String,
    /// How long opening took.
    pub millis: u128,
}

impl fmt::Debug for OpenedPhoto {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenedPhoto")
            .field("file_name", &self.file_name)
            .field("kind", &self.kind)
            .field("full_size", &(self.full_width, self.full_height))
            .field("working_size", &(self.renderer.width, self.renderer.height))
            .field("reduced", &self.reduced)
            .field("sidecar_error", &self.sidecar_error)
            .finish_non_exhaustive()
    }
}

/// Everything an open job needs, captured by value.
pub struct Request {
    pub photo_id: i64,
    pub path: PathBuf,
    /// The catalogue root (each job opens its own `Catalog`).
    pub root: PathBuf,
    pub gpu: Arc<Gpu>,
    pub cache: Option<Arc<FrameCache>>,
    pub quality: Quality,
    pub max_edge: u32,
    /// This request's generation, and the latest one asked for.
    pub generation: u64,
    pub latest: Arc<AtomicU64>,
}

/// The open job: answers `Msg::Opened` for this request's generation.
pub fn run(req: Request) -> Msg {
    let (photo_id, generation) = (req.photo_id, req.generation);
    let result = if req.latest.load(Ordering::SeqCst) != generation {
        Err(SUPERSEDED.to_owned())
    } else {
        open_photo(&req).map(Box::new)
    };
    Msg::Opened {
        photo_id,
        generation,
        result,
    }
}

fn open_photo(req: &Request) -> Result<OpenedPhoto, String> {
    let started = Instant::now();
    let catalog =
        Catalog::open(&req.root).map_err(|e| format!("The catalogue could not be read ({e})."))?;
    // An edit made on another machine shows on reopen; a damaged sidecar is recorded and
    // skipped, never reconciled as if absent.
    let sidecar_error = match edits::reconcile_on_open(&catalog, req.photo_id, &req.path) {
        Ok(OnOpen::Reconciled(_)) => None,
        Ok(OnOpen::Damaged(message)) => Some(message),
        Err(e) => {
            log::warn!(
                "could not reconcile the sidecar of photo {}: {e}",
                req.photo_id
            );
            None
        }
    };
    let settings = edits::get_settings(&catalog, req.photo_id)
        .map_err(|e| format!("The photo's edits could not be read ({e})."))?
        .unwrap_or_else(EditSettings::default_settings);
    drop(catalog);
    let develop = DevelopSettings::from_settings(&settings);
    let opened = match &req.cache {
        Some(cache) => open_cached(&req.path, req.quality, &develop, req.max_edge, cache),
        None => lf_raw::open::open(&req.path, req.quality, &develop, req.max_edge),
    }
    .map_err(|e| e.to_string())?;
    let sample = super::auto::Sample::of(&opened.picture);
    let renderer = lf_raw::render::renderer_for(&req.gpu, &opened).map_err(|e| e.to_string())?;
    let Opened {
        kind,
        exif,
        full_width,
        full_height,
        reduced,
        ..
    } = opened;
    let millis = started.elapsed().as_millis();
    log::info!("opened {kind:?} {full_width}×{full_height} in {millis} ms");
    Ok(OpenedPhoto {
        renderer,
        kind,
        exif,
        full_width,
        full_height,
        reduced,
        sample,
        as_shot_kelvin: lf_raw::open::as_shot_kelvin(&req.path),
        develop,
        settings,
        sidecar_error,
        file_name: file_name(&req.path),
        millis,
    })
}

/// Decodes a neighbour into the frame cache (RAW only) so stepping to it is quick; skipped
/// when another open was asked for since it was queued.
pub fn preopen(req: Request) -> Msg {
    let photo_id = req.photo_id;
    if req.latest.load(Ordering::SeqCst) != req.generation {
        return Msg::Preopened {
            photo_id,
            result: Ok(false),
        };
    }
    let result = (|| {
        let Some(cache) = &req.cache else {
            return Ok(false);
        };
        let settings = Catalog::open(&req.root)
            .and_then(|c| edits::get_settings(&c, photo_id))
            .map_err(|e| e.to_string())?
            .unwrap_or_else(EditSettings::default_settings);
        let develop = DevelopSettings::from_settings(&settings);
        open_cached(&req.path, req.quality, &develop, req.max_edge, cache)
            .map(|_| true)
            .map_err(|e| e.to_string())
    })();
    Msg::Preopened { photo_id, result }
}

/// The file's name for the info strip.
pub fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_cache(dir: &Path) -> FrameCache {
        FrameCache::open(&dir.join("frames"), 256 << 20).unwrap()
    }

    fn cached_entries(dir: &Path) -> usize {
        std::fs::read_dir(dir.join("frames"))
            .map(|d| {
                d.filter_map(Result::ok)
                    .filter(|e| e.path().extension().is_some_and(|x| x == "lff"))
                    .count()
            })
            .unwrap_or(0)
    }

    #[test]
    fn an_eight_bit_file_bypasses_the_frame_cache() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photo.jpg");
        image::RgbImage::from_fn(40, 30, |x, y| image::Rgb([x as u8 * 6, y as u8 * 8, 90]))
            .save(&path)
            .unwrap();
        let cache = temp_cache(dir.path());
        let settings = DevelopSettings::from_settings(&EditSettings::default_settings());
        let opened = open_cached(&path, Quality::Fast, &settings, 6144, &cache).unwrap();
        assert_eq!(opened.kind, Kind::Jpeg);
        assert_eq!((opened.full_width, opened.full_height), (40, 30));
        assert!(matches!(opened.picture, Picture::EightBit(_)));
        assert_eq!(cached_entries(dir.path()), 0);
    }

    /// A corpus RAW goes through the cache: the second open is served from it. Skipped when
    /// the corpus has not been fetched, unless `LF_REQUIRE_SAMPLES=1` (CI, when the whole
    /// corpus is present), where a missing file fails.
    #[test]
    fn a_raw_is_stored_then_served_from_the_frame_cache() {
        let corpus = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/raw-samples/files/nikon-d40.nef");
        if !corpus.exists() {
            assert!(
                std::env::var("LF_REQUIRE_SAMPLES").as_deref() != Ok("1"),
                "LF_REQUIRE_SAMPLES=1 but {} is missing",
                corpus.display()
            );
            eprintln!("RAW corpus not fetched: skipped");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let cache = temp_cache(dir.path());
        let settings = DevelopSettings::from_settings(&EditSettings::default_settings());
        let first = open_cached(&corpus, Quality::Fast, &settings, 1024, &cache).unwrap();
        assert_eq!(first.kind, Kind::Raw);
        assert_eq!(cached_entries(dir.path()), 1);
        let again = open_cached(&corpus, Quality::Fast, &settings, 1024, &cache).unwrap();
        assert_eq!(
            (again.full_width, again.full_height, again.reduced),
            (first.full_width, first.full_height, first.reduced)
        );
        assert_eq!(again.exif.model, first.exif.model);
        assert_eq!(cached_entries(dir.path()), 1);
    }
}
