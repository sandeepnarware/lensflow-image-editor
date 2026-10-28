//! Export edited photos to disk: JPEG, PNG, lossless WebP or 16-bit TIFF, resized, sharpened
//! for the screen, watermarked, renamed, with EXIF and an sRGB colour profile.
//!
//! Each photo is opened at its full size (an export never uses a reduced frame), developed
//! with the settings in the catalogue, rendered through the GPU pipeline and encoded. JPEG,
//! PNG and WebP take the 8-bit render; TIFF takes the half-float render (clamped to 0..1 and
//! scaled to 16 bits only at the encode). After resizing, [`crate::finish`] sharpens and
//! stamps the watermark. Every file carries an sRGB ICC profile; EXIF goes into JPEG, PNG
//! (`eXIf`) and WebP, not TIFF (the encoder cannot add it).
//!
//! A run never stops at one bad photo: each failure is listed in the [`ExportReport`] and the
//! rest are exported. Only the up-front checks (name collisions, existing files when not
//! overwriting, an output landing on a catalogued photo) fail the whole run, before anything
//! is written. A lost graphics device stops it where it is ([`ExportReport::device_lost`]).
//! The progress callback can cancel after the current photo.

use std::io::Write;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::codecs::webp::WebPEncoder;
use image::{DynamicImage, ImageBuffer, ImageEncoder};
use lf_pipeline::Gpu;
use lf_raw::demosaic::Quality;
use lf_raw::open::{Kind, Picture, open};
use lf_settings::{DevelopSettings, EditSettings};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::db::{Catalog, CatalogError, Result};
use crate::edits;
use crate::exif_embed::{build_exif, insert_exif, insert_xmp, xmp_segment};
use crate::finish::{self, Pixels, Prepared, Sharpen, Watermark};
use crate::model::{now, ts};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Format {
    /// `quality` 1..=100.
    Jpeg { quality: u8 },
    /// 8-bit.
    Png,
    /// 8-bit, lossless.
    WebP,
    /// 8-bit, lossy; `quality` 1..=100.
    WebPLossy { quality: u8 },
    /// 8-bit AVIF; `quality` 1..=100.
    Avif { quality: u8 },
    /// 16-bit RGB.
    Tiff16,
}

impl Format {
    fn extension(self) -> &'static str {
        match self {
            Format::Jpeg { .. } => "jpg",
            Format::Png => "png",
            Format::WebP | Format::WebPLossy { .. } => "webp",
            Format::Avif { .. } => "avif",
            Format::Tiff16 => "tif",
        }
    }
}

/// The exported size. Edge and megapixel sizes never enlarge a photo; `Percent` scales as
/// asked.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Size {
    Original,
    LongEdge(u32),
    ShortEdge(u32),
    Megapixels(f32),
    Percent(f32),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Destination {
    Folder(PathBuf),
}

/// A file-name pattern (without the extension): `{name}` the original stem, `{seq}` 1-based
/// and zero-padded to the width of the count, `{date}` `YYYY-MM-DD` (capture date, else the
/// file's modified date), `{camera}` make and model with spaces as `-` (or `unknown`),
/// `{title}` the catalogue title (or the original stem).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pattern(pub String);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Metadata {
    /// The source JPEG's EXIF byte for byte, or a minimal EXIF from the file's facts.
    All,
    /// Artist and Copyright only.
    CopyrightOnly,
    None,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExportSpec {
    pub format: Format,
    pub size: Size,
    pub destination: Destination,
    pub rename: Pattern,
    pub metadata: Metadata,
    /// When false, an existing file is [`CatalogError::Exists`], never replaced.
    pub overwrite: bool,
    /// Sharpening for the screen, after resizing. (Absent in specs saved before it existed.)
    #[serde(default)]
    pub sharpen: Sharpen,
    #[serde(default)]
    pub watermark: Option<Watermark>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Exported {
    pub photo_id: i64,
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
}

/// What a run did: the photos written and the photos that could not be exported (in the
/// order asked), and whether the progress callback cancelled it.
#[derive(Debug)]
pub struct ExportReport {
    pub exported: Vec<Exported>,
    pub failed: Vec<(i64, CatalogError)>,
    pub cancelled: bool,
    /// The graphics device was lost: the run stopped there (see [`run`]).
    pub device_lost: bool,
}

/// A row of `export_jobs`.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub id: i64,
    /// `running`, `done` (every photo exported), `failed` (the run was refused up front, or
    /// some photos failed; `error` says which) or `cancelled`.
    pub status: String,
    pub done: i64,
    pub total: i64,
    pub error: Option<String>,
}

/// The most recent export job.
pub fn last_job(catalog: &Catalog) -> Result<Option<Job>> {
    Ok(catalog
        .conn
        .query_row(
            "SELECT id, status, done, total, error FROM export_jobs ORDER BY id DESC LIMIT 1",
            [],
            |r| {
                Ok(Job {
                    id: r.get(0)?,
                    status: r.get(1)?,
                    done: r.get(2)?,
                    total: r.get(3)?,
                    error: r.get(4)?,
                })
            },
        )
        .optional()?)
}

/// A file stem that is safe on Windows: reserved device names get a `_` suffix and trailing
/// dots and spaces go.
pub fn safe_stem(stem: &str) -> String {
    let t = stem.trim_end_matches(['.', ' ']);
    let base = t.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((base.starts_with("COM") || base.starts_with("LPT"))
            && base.len() == 4
            && matches!(base.as_bytes()[3], b'1'..=b'9'));
    if reserved {
        format!("{t}_")
    } else {
        t.to_string()
    }
}

/// `pattern` with its tokens filled in and path-illegal characters replaced by `-`.
pub fn file_name(
    pattern: &str,
    name: &str,
    seq: usize,
    count: usize,
    date: &str,
    camera: &str,
) -> String {
    let width = count.max(1).to_string().len();
    let camera = camera.trim();
    let camera = if camera.is_empty() {
        "unknown".to_string()
    } else {
        camera.split_whitespace().collect::<Vec<_>>().join("-")
    };
    let out: String = pattern
        .replace("{name}", name)
        .replace("{seq}", &format!("{seq:0width$}"))
        .replace("{date}", date)
        .replace("{camera}", &camera)
        .chars()
        .map(|c| {
            if c.is_control() || "\\/:*?\"<>|".contains(c) {
                '-'
            } else {
                c
            }
        })
        .collect();
    let out = out.trim_end_matches(['.', ' ']).to_string();
    if out.trim().is_empty() {
        "photo".to_string()
    } else {
        out
    }
}

/// The size `(w, h)` becomes under `size`.
fn target_size(size: Size, w: u32, h: u32) -> (u32, u32) {
    let (wf, hf) = (f64::from(w), f64::from(h));
    let scale = match size {
        Size::Original => 1.0,
        Size::LongEdge(n) => (f64::from(n) / wf.max(hf)).min(1.0),
        Size::ShortEdge(n) => (f64::from(n) / wf.min(hf)).min(1.0),
        Size::Megapixels(m) => (f64::from(m) * 1e6 / (wf * hf)).sqrt().min(1.0),
        Size::Percent(p) => f64::from(p) / 100.0,
    };
    if scale == 1.0 || !scale.is_finite() || scale <= 0.0 {
        return (w, h);
    }
    (
        ((wf * scale).round() as u32).max(1),
        ((hf * scale).round() as u32).max(1),
    )
}

struct PhotoRow {
    path: String,
    camera: Option<String>,
    captured_at: Option<String>,
    mtime: i64,
    meta: crate::meta::Meta,
}

fn photo_row(catalog: &Catalog, id: i64) -> Result<PhotoRow> {
    let meta = catalog.photo(id)?.map(|p| p.meta).unwrap_or_default();
    catalog
        .conn
        .query_row(
            "SELECT path, camera, captured_at, mtime FROM photos WHERE id = ?1",
            [id],
            |r| {
                Ok(PhotoRow {
                    path: r.get(0)?,
                    camera: r.get(1)?,
                    captured_at: r.get(2)?,
                    mtime: r.get(3)?,
                    meta: meta.clone(),
                })
            },
        )
        .optional()?
        .ok_or_else(|| CatalogError::NotFound(format!("photo {id}")))
}

fn date_of(row: &PhotoRow) -> String {
    if let Some(d) = row.captured_at.as_deref().and_then(|c| c.get(..10))
        && chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok()
    {
        return d.to_string();
    }
    chrono::DateTime::from_timestamp_millis(row.mtime).map_or_else(
        || "unknown".to_string(),
        |t| t.format("%Y-%m-%d").to_string(),
    )
}

/// Where photo `id` goes, and its row.
fn plan_one(
    catalog: &Catalog,
    id: i64,
    seq: usize,
    count: usize,
    spec: &ExportSpec,
) -> Result<(PhotoRow, PathBuf)> {
    let row = photo_row(catalog, id)?;
    let stem = Path::new(&row.path)
        .file_stem()
        .map_or_else(|| "photo".into(), |s| s.to_string_lossy().into_owned());
    // `{title}`: the catalogue title (else the file name), filled in before the other tokens.
    let title = row
        .meta
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map_or_else(|| safe_stem(&stem), str::to_owned);
    let pattern = spec.rename.0.replace("{title}", &title);
    let name = file_name(
        &pattern,
        &safe_stem(&stem),
        seq,
        count,
        &date_of(&row),
        row.camera.as_deref().unwrap_or(""),
    );
    let Destination::Folder(dir) = &spec.destination;
    // The whole name too: a pattern can be a reserved name by itself (`CON`, `aux-{seq}`...).
    let path = dir.join(format!("{}.{}", safe_stem(&name), spec.format.extension()));
    Ok((row, path))
}

/// Every output path, resolved before anything is rendered: two photos on one path is
/// [`CatalogError::NameCollision`], and (unless overwriting) an existing file is
/// [`CatalogError::Exists`], so nothing is written unless every path is free.
fn plan(catalog: &Catalog, ids: &[i64], spec: &ExportSpec) -> Result<Vec<(PhotoRow, PathBuf)>> {
    let mut seen: std::collections::HashMap<String, i64> = Default::default();
    let mut out = Vec::with_capacity(ids.len());
    for (i, id) in ids.iter().enumerate() {
        let (row, path) = plan_one(catalog, *id, i + 1, ids.len(), spec)?;
        // Case-insensitive: Windows and macOS volumes treat these as one file.
        let key = path.to_string_lossy().to_lowercase();
        if let Some(first) = seen.insert(key, *id) {
            return Err(CatalogError::NameCollision {
                path,
                photos: (first, *id),
            });
        }
        out.push((row, path));
    }
    // Before the existing-file check, and whatever `overwrite` says: the desktop never
    // rewrites a source file.
    protect_originals(catalog, out.iter().map(|(_, p)| p.as_path()))?;
    if !spec.overwrite
        && let Some((_, path)) = out.iter().find(|(_, p)| p.exists())
    {
        return Err(CatalogError::Exists(path.clone()));
    }
    Ok(out)
}

/// The key two paths are compared by: canonical where the file (or else its folder) exists,
/// lexical otherwise, and lower-cased (Windows and macOS volumes ignore case; being stricter
/// elsewhere only refuses more).
fn path_key(path: &Path) -> String {
    let resolved = std::fs::canonicalize(path).ok().or_else(|| {
        let parent = path.parent()?;
        let parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        Some(std::fs::canonicalize(parent).ok()?.join(path.file_name()?))
    });
    let resolved = resolved.unwrap_or_else(|| {
        // Neither exists: a lexical clean-up (`.` dropped, `..` folded).
        let mut out = PathBuf::new();
        for c in path.components() {
            match c {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    out.pop();
                }
                c => out.push(c),
            }
        }
        out
    });
    resolved.to_string_lossy().to_lowercase()
}

/// A sidecar's name (`<photo file>.lensflow.json`), compared without case.
fn is_sidecar(path: &Path) -> bool {
    path.file_name().is_some_and(|n| {
        n.to_string_lossy()
            .to_lowercase()
            .ends_with(".lensflow.json")
    })
}

/// [`CatalogError::WouldOverwriteOriginal`] for the first output that is a sidecar or the
/// path of any catalogued photo (the whole catalogue, not only the photos being exported).
/// Only photos whose file name matches an output's are resolved on disk, so a large
/// catalogue costs one string pass.
fn protect_originals<'a>(
    catalog: &Catalog,
    outputs: impl Iterator<Item = &'a Path> + Clone,
) -> Result<()> {
    if let Some(path) = outputs.clone().find(|p| is_sidecar(p)) {
        return Err(CatalogError::WouldOverwriteOriginal(path.to_path_buf()));
    }
    let lower_name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().to_lowercase());
    let names: std::collections::HashSet<String> = outputs.clone().filter_map(lower_name).collect();
    let mut stmt = catalog.conn.prepare("SELECT path FROM photos")?;
    let candidates: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .filter_map(std::result::Result::ok)
        .filter(|p| lower_name(Path::new(p)).is_some_and(|n| names.contains(&n)))
        .collect();
    if candidates.is_empty() {
        return Ok(());
    }
    let sources: std::collections::HashSet<String> =
        candidates.iter().map(|p| path_key(Path::new(p))).collect();
    match outputs.into_iter().find(|p| sources.contains(&path_key(p))) {
        Some(path) => Err(CatalogError::WouldOverwriteOriginal(path.to_path_buf())),
        None => Ok(()),
    }
}

fn export_one(
    gpu: &Arc<Gpu>,
    catalog: &Catalog,
    id: i64,
    row: &PhotoRow,
    out_path: PathBuf,
    spec: &ExportSpec,
    watermark: Option<&Prepared>,
) -> Result<Exported> {
    let fail =
        |what: &dyn std::fmt::Display, path: &str| CatalogError::Export(format!("{path}: {what}"));
    let src = Path::new(&row.path);
    let Destination::Folder(dir) = &spec.destination;

    let settings = edits::get_settings(catalog, id)?.unwrap_or_else(EditSettings::default_settings);
    let develop = DevelopSettings::from_settings(&settings);
    let max_edge = gpu.max_edge();
    // A decoder panic fails this photo, not the run.
    let opened = std::panic::catch_unwind(|| {
        #[cfg(feature = "test-hooks")]
        if src.file_stem().is_some_and(|s| s == "lf-test-panic") {
            panic!("test hook: the decoder panics on {}", src.display());
        }
        open(src, Quality::Best, &develop, max_edge)
    })
    .map_err(|_| {
        log::error!("the decoder crashed exporting {}", row.path);
        fail(&crate::import::DECODER_CRASHED, &row.path)
    })?
    .map_err(|e| fail(&e, &row.path))?;
    if opened.reduced {
        return Err(fail(
            &format!(
                "{}x{} is larger than this graphics device can export at full size",
                opened.full_width, opened.full_height
            ),
            &row.path,
        ));
    }
    // A lost device is not this photo's fault: it stops the run (see `run`).
    let gpu_fail = |e: lf_pipeline::PipelineError| match e {
        lf_pipeline::PipelineError::DeviceLost => CatalogError::DeviceLost,
        e => fail(&e, &row.path),
    };
    let mut renderer = lf_raw::render::renderer_for(gpu, &opened).map_err(gpu_fail)?;
    let pixels = match spec.format {
        Format::Tiff16 => {
            let f = renderer.render_f16(&settings).map_err(gpu_fail)?;
            let data: Vec<u16> = f
                .data
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| {
                    [0, 1, 2].map(|i| (p[i].to_f32().clamp(0.0, 1.0) * 65535.0).round() as u16)
                })
                .collect();
            Pixels::Sixteen(
                ImageBuffer::from_raw(f.width, f.height, data).expect("rgb16 buffer size"),
            )
        }
        _ => {
            let img = renderer.render(&settings).map_err(gpu_fail)?;
            Pixels::Eight(DynamicImage::ImageRgba8(img).to_rgb8())
        }
    };
    drop(renderer);
    let (w, h) = pixels.dimensions();
    let (tw, th) = target_size(spec.size, w, h);
    let limit = gpu.max_edge();
    if tw > limit || th > limit {
        return Err(CatalogError::TooLarge {
            width: tw,
            height: th,
            limit,
        });
    }
    let mut pixels = finish::sharpen(pixels.resized(tw, th), spec.sharpen);
    if let Some(mark) = watermark {
        finish::watermark(&mut pixels, mark);
    }

    // The EXIF as a JPEG APP1 segment; PNG and WebP take the TIFF part inside it. The
    // catalogue's creator and copyright ([`crate::meta`]) win over the file's.
    let mut facts = opened.exif.clone();
    if let Some(c) = &row.meta.creator {
        facts.artist = Some(c.clone());
    }
    if let Some(c) = &row.meta.copyright {
        facts.copyright = Some(c.clone());
    }
    let app1 = match spec.metadata {
        Metadata::None => None,
        Metadata::CopyrightOnly => build_exif(&facts, true),
        Metadata::All => match (&opened.kind, &opened.jpeg_exif_bytes) {
            (Kind::Jpeg, Some(kept)) => Some(kept.clone()),
            _ => build_exif(&facts, false),
        },
    };
    // Title, caption, keywords (and creator, copyright) as XMP, in JPEGs.
    let xmp = match spec.metadata {
        Metadata::None => None,
        Metadata::CopyrightOnly => xmp_segment(&row.meta, true),
        Metadata::All => xmp_segment(&row.meta, false),
    };
    let exif = app1.as_deref().and_then(finish::exif_payload);
    // The XMP packet alone, for the formats that hold it in a chunk or tag of their own.
    let packet = xmp.as_deref().map(crate::exif_embed::xmp_packet);
    let icc = finish::srgb_icc();
    let encode_failed = |e: &dyn std::fmt::Display| fail(e, &row.path);
    // Every encoder here takes a profile; PNG and WebP take EXIF too.
    fn tagged<E: ImageEncoder>(
        mut encoder: E,
        icc: &[u8],
        exif: Option<&Vec<u8>>,
        image: DynamicImage,
    ) -> image::ImageResult<()> {
        // A refusal is "this format cannot hold it": the photo is still exported.
        let _ = encoder.set_icc_profile(icc.to_vec());
        if let Some(exif) = exif {
            let _ = encoder.set_exif_metadata(exif.clone());
        }
        image.write_with_encoder(encoder)
    }
    let mut bytes = Vec::new();
    match (spec.format, pixels) {
        (Format::Jpeg { quality }, Pixels::Eight(img)) => {
            let encoder = JpegEncoder::new_with_quality(&mut bytes, quality.clamp(1, 100));
            // JPEG's EXIF goes in as the APP1 segment itself, byte for byte, below.
            tagged(encoder, &icc, None, DynamicImage::ImageRgb8(img))
                .map_err(|e| encode_failed(&e))?;
            if let Some(app1) = &app1 {
                bytes = insert_exif(&bytes, app1);
            }
            if let Some(xmp) = &xmp {
                bytes = insert_xmp(&bytes, xmp);
            }
        }
        (Format::Png, Pixels::Eight(img)) => {
            tagged(
                PngEncoder::new(&mut bytes),
                &icc,
                exif.as_ref(),
                DynamicImage::ImageRgb8(img),
            )
            .map_err(|e| encode_failed(&e))?;
            if let Some(packet) = packet {
                bytes = crate::containers::png_with_xmp(&bytes, packet);
            }
        }
        (Format::WebP, Pixels::Eight(img)) => {
            let (w, h) = img.dimensions();
            let mut plain = Vec::new();
            WebPEncoder::new_lossless(&mut plain)
                .write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgb8)
                .map_err(|e| encode_failed(&e))?;
            bytes = crate::containers::webp_with_metadata(
                &plain,
                w,
                h,
                Some(&icc),
                exif.as_deref(),
                packet,
            );
        }
        (Format::WebPLossy { quality }, Pixels::Eight(img)) => {
            let (w, h) = img.dimensions();
            let plain = webp::Encoder::from_rgb(img.as_raw(), w, h)
                .encode(f32::from(quality.clamp(1, 100)))
                .to_vec();
            bytes = crate::containers::webp_with_metadata(
                &plain,
                w,
                h,
                Some(&icc),
                exif.as_deref(),
                packet,
            );
        }
        (Format::Avif { quality }, Pixels::Eight(img)) => {
            // AVIF says "sRGB" in its own colour box; the encoder takes no EXIF or XMP.
            let (w, h) = img.dimensions();
            image::codecs::avif::AvifEncoder::new_with_speed_quality(
                &mut bytes,
                AVIF_SPEED,
                quality.clamp(1, 100),
            )
            .write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgb8)
            .map_err(|e| encode_failed(&e))?;
        }
        (Format::Tiff16, Pixels::Sixteen(img)) => {
            let (w, h) = img.dimensions();
            let copyright_only = spec.metadata == Metadata::CopyrightOnly;
            let tags = if spec.metadata == Metadata::None {
                crate::containers::TiffTags::default()
            } else {
                crate::containers::TiffTags {
                    artist: facts.artist.clone(),
                    copyright: facts.copyright.clone(),
                    description: row.meta.title.clone().filter(|_| !copyright_only),
                    make: facts.make.clone().filter(|_| !copyright_only),
                    model: facts.model.clone().filter(|_| !copyright_only),
                }
            };
            bytes = crate::containers::tiff16(w, h, img.as_raw(), &icc, packet, &tags)
                .map_err(|e| encode_failed(&e))?;
        }
        _ => unreachable!("the render matches the format"),
    }
    // Keep the match on `opened.picture` honest if a new variant appears.
    let _: &Picture = &opened.picture;

    std::fs::create_dir_all(dir)?;
    write_output(&out_path, &bytes, spec.overwrite)?;
    Ok(Exported {
        photo_id: id,
        path: out_path,
        width: tw,
        height: th,
    })
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The prefix of an export's temp files (`.lfexport-<name>.<pid>-<n>.tmp`): its own, so the
/// app's sweep of unfinished exports never matches a sidecar's temp file
/// (`.<photo>.lensflow.json.<pid>-<n>.tmp`) in a folder that also holds catalogued photos.
/// rav1e's speed for AVIF exports (1 slowest, 10 fastest): a photo in seconds, not minutes.
const AVIF_SPEED: u8 = 7;

pub const TEMP_PREFIX: &str = ".lfexport-";

/// The hidden temp file a replacing write of `path` goes through.
pub fn temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!(
        "{TEMP_PREFIX}{name}.{}-{}.tmp",
        std::process::id(),
        TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Write `bytes` to `path`. Overwriting goes through a temp file in the same folder and a
/// rename, so a failure never leaves a truncated file where the old one was. Otherwise the
/// file is created new (an existing one is [`CatalogError::Exists`]) and removed again if the
/// write fails.
fn write_output(path: &Path, bytes: &[u8], overwrite: bool) -> Result<()> {
    if overwrite {
        let tmp = temp_path(path);
        let result = (|| -> Result<()> {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(bytes)?;
            f.sync_all()?;
            drop(f);
            std::fs::rename(&tmp, path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        return result;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                CatalogError::Exists(path.to_path_buf())
            } else {
                CatalogError::Io(e)
            }
        })?;
    if let Err(e) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(e.into());
    }
    Ok(())
}

/// Export `photo_ids` in order. `progress(done, total)` is called before the first photo and
/// after each one (exported or failed); returning `Break` cancels the run after the photo
/// just finished. A photo that fails is listed in [`ExportReport::failed`] and the run goes
/// on. Only the up-front checks fail the whole run (`Err`), before anything is written: two
/// photos on one path ([`CatalogError::NameCollision`]) or, unless overwriting, an existing
/// file ([`CatalogError::Exists`]); and, whatever `overwrite` says, an output that would land
/// on any catalogued photo or a sidecar ([`CatalogError::WouldOverwriteOriginal`]). A
/// graphics device lost before or during a photo stops the run there: the report comes back
/// with `device_lost` set and what was written so far (the job row is `failed` with
/// [`CatalogError::DeviceLost`]'s message). The run is recorded in `export_jobs`.
pub fn run(
    gpu: Arc<Gpu>,
    catalog: &Catalog,
    photo_ids: &[i64],
    spec: &ExportSpec,
    progress: &mut dyn FnMut(usize, usize) -> ControlFlow<()>,
) -> Result<ExportReport> {
    let total = photo_ids.len();
    let job = {
        let tx = catalog.write_tx()?;
        tx.execute(
            "INSERT INTO export_jobs (created_at, status, spec, done, total)
             VALUES (?1, 'running', ?2, 0, ?3)",
            rusqlite::params![ts(now()), serde_json::to_string(spec)?, total as i64],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        id
    };
    let finish = |status: &str, done: usize, error: Option<String>| -> Result<()> {
        catalog.conn.execute(
            "UPDATE export_jobs SET status = ?2, done = ?3, error = ?4 WHERE id = ?1",
            rusqlite::params![job, status, done as i64, error],
        )?;
        Ok(())
    };
    let mut report = ExportReport {
        exported: Vec::with_capacity(total),
        failed: Vec::new(),
        cancelled: false,
        device_lost: false,
    };
    // The plan and the watermark (its logo read once) are checked before anything is written.
    let ready = plan(catalog, photo_ids, spec).and_then(|p| {
        let mark = spec.watermark.as_ref().map(Prepared::new).transpose()?;
        Ok((p, mark))
    });
    let (planned, watermark) = match ready {
        Ok(ready) => ready,
        Err(e) => {
            finish("failed", 0, Some(e.to_string()))?;
            return Err(e);
        }
    };
    report.cancelled = progress(0, total).is_break();
    for (i, (row, path)) in planned.into_iter().enumerate() {
        if report.cancelled {
            break;
        }
        let id = photo_ids[i];
        let device_lost = |mut report: ExportReport| -> Result<ExportReport> {
            log::error!(
                "the graphics device was lost exporting photo {id}; stopping after {} photos",
                report.exported.len()
            );
            report.device_lost = true;
            let e = CatalogError::DeviceLost;
            finish("failed", report.exported.len(), Some(e.to_string()))?;
            Ok(report)
        };
        if gpu.is_lost() {
            return device_lost(report);
        }
        match export_one(&gpu, catalog, id, &row, path, spec, watermark.as_ref()) {
            Ok(e) => report.exported.push(e),
            Err(CatalogError::DeviceLost) => return device_lost(report),
            Err(_) if gpu.is_lost() => return device_lost(report),
            Err(e) => {
                log::warn!("export of photo {id} ({}) failed: {e}", row.path);
                report.failed.push((id, e));
            }
        }
        report.cancelled = progress(i + 1, total).is_break();
    }
    let done = report.exported.len();
    let error = report.failed.first().map(|(id, e)| {
        format!(
            "{} of {total} photos could not be exported; the first (photo {id}): {e}",
            report.failed.len()
        )
    });
    let status = if report.cancelled {
        "cancelled"
    } else if report.failed.is_empty() {
        "done"
    } else {
        "failed"
    };
    finish(status, done, error)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_names_are_never_export_targets() {
        let tmp = tempfile::tempdir().unwrap();
        let c = Catalog::open(tmp.path()).unwrap();
        let dir = tmp.path().to_path_buf();
        for name in ["IMG_1.jpg.lensflow.json", "x.LENSFLOW.JSON"] {
            let p = dir.join(name);
            assert!(matches!(
                protect_originals(&c, [p.as_path()].into_iter()),
                Err(CatalogError::WouldOverwriteOriginal(_))
            ));
        }
        let fine = dir.join("IMG_1.jpg");
        assert!(protect_originals(&c, [fine.as_path()].into_iter()).is_ok());
    }

    #[test]
    fn path_keys_ignore_case_and_dots() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("IMG_1.JPG"), b"x").unwrap();
        let a = path_key(&dir.path().join("IMG_1.JPG"));
        assert_eq!(a, path_key(&dir.path().join("IMG_1.jpg")));
        assert_eq!(a, path_key(&dir.path().join(".").join("img_1.jpg")));
        let missing = Path::new("no-such-dir/sub/../x.jpg");
        assert_eq!(
            path_key(missing),
            "no-such-dir/x.jpg".replace('/', std::path::MAIN_SEPARATOR_STR)
        );
    }
}
