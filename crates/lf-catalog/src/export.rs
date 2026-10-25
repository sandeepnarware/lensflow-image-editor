//! Export edited photos to disk: JPEG, PNG or 16-bit TIFF, resized, renamed and with EXIF.
//!
//! Each photo is opened at its full size (an export never uses a reduced frame), developed
//! with the settings in the catalogue, rendered through the GPU pipeline and encoded. JPEG
//! and PNG take the 8-bit render; TIFF takes the half-float render (clamped to 0..1 and
//! scaled to 16 bits only at the encode). EXIF is written for JPEG output only: PNG and TIFF
//! exports carry none in this version.

use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::imageops::{FilterType, resize};
use image::{DynamicImage, ImageBuffer, ImageFormat, Rgb, RgbImage};
use lf_pipeline::Gpu;
use lf_raw::demosaic::Quality;
use lf_raw::open::{Kind, Picture, open};
use lf_settings::{DevelopSettings, EditSettings};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::db::{Catalog, CatalogError, Result};
use crate::edits;
use crate::exif_embed::{build_exif, insert_exif};
use crate::model::{now, ts};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Format {
    /// `quality` 1..=100.
    Jpeg { quality: u8 },
    /// 8-bit.
    Png,
    /// 16-bit RGB.
    Tiff16,
}

impl Format {
    fn extension(self) -> &'static str {
        match self {
            Format::Jpeg { .. } => "jpg",
            Format::Png => "png",
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
/// file's modified date), `{camera}` make and model with spaces as `-` (or `unknown`).
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
}

#[derive(Clone, Debug, PartialEq)]
pub struct Exported {
    pub photo_id: i64,
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
}

/// A row of `export_jobs`.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub id: i64,
    /// `running`, `done` or `failed`.
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
}

fn photo_row(catalog: &Catalog, id: i64) -> Result<PhotoRow> {
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

enum Pixels {
    Eight(RgbImage),
    Sixteen(ImageBuffer<Rgb<u16>, Vec<u16>>),
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
    let name = file_name(
        &spec.rename.0,
        &safe_stem(&stem),
        seq,
        count,
        &date_of(&row),
        row.camera.as_deref().unwrap_or(""),
    );
    let Destination::Folder(dir) = &spec.destination;
    let path = dir.join(format!("{name}.{}", spec.format.extension()));
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
    if !spec.overwrite
        && let Some((_, path)) = out.iter().find(|(_, p)| p.exists())
    {
        return Err(CatalogError::Exists(path.clone()));
    }
    Ok(out)
}

fn export_one(
    gpu: &Arc<Gpu>,
    catalog: &Catalog,
    id: i64,
    row: &PhotoRow,
    out_path: PathBuf,
    spec: &ExportSpec,
) -> Result<Exported> {
    let fail =
        |what: &dyn std::fmt::Display, path: &str| CatalogError::Export(format!("{path}: {what}"));
    let src = Path::new(&row.path);
    let Destination::Folder(dir) = &spec.destination;

    let settings = edits::get_settings(catalog, id)?.unwrap_or_else(EditSettings::default_settings);
    let opened = open(
        src,
        Quality::Best,
        &DevelopSettings::from_settings(&settings),
        gpu.max_edge(),
    )
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
    let mut renderer =
        lf_raw::render::renderer_for(gpu, &opened).map_err(|e| fail(&e, &row.path))?;
    let pixels = match spec.format {
        Format::Tiff16 => {
            let f = renderer
                .render_f16(&settings)
                .map_err(|e| fail(&e, &row.path))?;
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
            let img = renderer
                .render(&settings)
                .map_err(|e| fail(&e, &row.path))?;
            Pixels::Eight(DynamicImage::ImageRgba8(img).to_rgb8())
        }
    };
    drop(renderer);
    let (w, h) = match &pixels {
        Pixels::Eight(i) => i.dimensions(),
        Pixels::Sixteen(i) => i.dimensions(),
    };
    let (tw, th) = target_size(spec.size, w, h);
    let pixels = if (tw, th) == (w, h) {
        pixels
    } else {
        match pixels {
            Pixels::Eight(i) => Pixels::Eight(resize(&i, tw, th, FilterType::Lanczos3)),
            Pixels::Sixteen(i) => Pixels::Sixteen(resize(&i, tw, th, FilterType::Lanczos3)),
        }
    };

    let mut bytes = Vec::new();
    match (spec.format, pixels) {
        (Format::Jpeg { quality }, Pixels::Eight(img)) => {
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality.clamp(1, 100))
                .encode_image(&img)
                .map_err(|e| fail(&e, &row.path))?;
            let app1 = match spec.metadata {
                Metadata::None => None,
                Metadata::CopyrightOnly => build_exif(&opened.exif, true),
                Metadata::All => match (&opened.kind, &opened.jpeg_exif_bytes) {
                    (Kind::Jpeg, Some(kept)) => Some(kept.clone()),
                    _ => build_exif(&opened.exif, false),
                },
            };
            if let Some(app1) = app1 {
                bytes = insert_exif(&bytes, &app1);
            }
        }
        (Format::Png, Pixels::Eight(img)) => {
            DynamicImage::ImageRgb8(img)
                .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
                .map_err(|e| fail(&e, &row.path))?;
        }
        (Format::Tiff16, Pixels::Sixteen(img)) => {
            DynamicImage::ImageRgb16(img)
                .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Tiff)
                .map_err(|e| fail(&e, &row.path))?;
        }
        _ => unreachable!("the render matches the format"),
    }
    // Keep the match on `opened.picture` honest if a new variant appears.
    let _: &Picture = &opened.picture;

    std::fs::create_dir_all(dir)?;
    let mut file = std::fs::OpenOptions::new();
    file.write(true);
    if spec.overwrite {
        file.create(true).truncate(true);
    } else {
        file.create_new(true);
    }
    let mut file = file.open(&out_path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            CatalogError::Exists(out_path.clone())
        } else {
            CatalogError::Io(e)
        }
    })?;
    file.write_all(&bytes)?;
    Ok(Exported {
        photo_id: id,
        path: out_path,
        width: tw,
        height: th,
    })
}

/// Export `photo_ids` in order. `progress(done, total)` is called before the first photo and
/// after each. The first failure stops the run (files already written stay) and is recorded
/// in `export_jobs`.
pub fn run(
    gpu: Arc<Gpu>,
    catalog: &Catalog,
    photo_ids: &[i64],
    spec: &ExportSpec,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<Vec<Exported>> {
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
    let mut out = Vec::with_capacity(total);
    progress(0, total);
    let mut result = Ok(());
    let planned = match plan(catalog, photo_ids, spec) {
        Ok(p) => p,
        Err(e) => {
            result = Err(e);
            Vec::new()
        }
    };
    for (i, (row, path)) in planned.into_iter().enumerate() {
        match export_one(&gpu, catalog, photo_ids[i], &row, path, spec) {
            Ok(e) => {
                out.push(e);
                progress(i + 1, total);
            }
            Err(e) => {
                result = Err(e);
                break;
            }
        }
    }
    let (status, error) = match &result {
        Ok(()) => ("done", None),
        Err(e) => ("failed", Some(e.to_string())),
    };
    catalog.conn.execute(
        "UPDATE export_jobs SET status = ?2, done = ?3, error = ?4 WHERE id = ?1",
        rusqlite::params![job, status, out.len() as i64, error],
    )?;
    result.map(|()| out)
}
