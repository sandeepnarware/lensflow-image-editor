//! The AI tools that make a new photo, on the `Long` lane: AI Denoise, AI Upscale, Retouch
//! portrait and Remove background. The AI folder is made ready first (the runtime and the
//! tool's models downloaded once, [`lf_ai::Engine::prepare`]); then each photo is rendered at
//! full size with its edits (what the photo looks like), worked on by [`lf_ai`], and written
//! beside the original as a new file, which the library then catalogues:
//! `<name>-Denoised.tif`, `<name>-Upscaled-2x.tif`, `<name>-Retouched.tif` (16-bit TIFF with
//! the sRGB profile) or `<name>-Cutout.png` (8-bit with transparency). The original is never
//! touched. Everything runs on this computer.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use image::ImageEncoder as _;
use lf_ai::segment::Target;
use lf_ai::{Engine, Gray, Rgb};
use lf_catalog::{Catalog, edits};
use lf_pipeline::Gpu;
use lf_raw::demosaic::Quality;
use lf_settings::{DevelopSettings, EditSettings};

/// A tool and its setting.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AiTool {
    /// How much of the model's result to keep, 0..1.
    Denoise { strength: f32 },
    /// 2 or 4.
    Upscale { factor: usize },
    /// How much to smooth skin, 0..1.
    Retouch { amount: f32 },
    /// The main subject on a transparent background.
    Cutout,
    /// What a painted mask covers, filled from its surroundings (the mask comes with the run).
    Remove,
    /// A mask of the subject, the sky, the people or a part of them for the open photo,
    /// added in Develop.
    Mask { target: Target },
    /// Sharpness, faces and closed eyes scored into the catalogue ([`lf_ai::cull`]).
    Cull,
    /// Faces found, described and grouped into people in the catalogue
    /// ([`lf_ai::identity`], [`lf_catalog::people`]).
    FindPeople,
}

impl AiTool {
    /// The models it needs (the runtime comes with every tool).
    pub fn models(self) -> Vec<lf_ai::assets::Asset> {
        match self {
            AiTool::Denoise { .. } => lf_ai::restore::DENOISE_MODELS.to_vec(),
            AiTool::Upscale { .. } => lf_ai::restore::UPSCALE_MODELS.to_vec(),
            AiTool::Retouch { .. } => lf_ai::faces::FACE_MODELS.to_vec(),
            AiTool::Cutout => lf_ai::segment::SUBJECT_MODELS.to_vec(),
            AiTool::Remove => lf_ai::inpaint::INPAINT_MODELS.to_vec(),
            AiTool::Mask { target } => target.models().to_vec(),
            AiTool::Cull => lf_ai::cull::CULL_MODELS.to_vec(),
            AiTool::FindPeople => lf_ai::identity::IDENTITY_MODELS.to_vec(),
        }
    }

    /// The tool's name, as the dialog's title starts.
    pub fn name(self) -> &'static str {
        match self {
            AiTool::Denoise { .. } => "AI Denoise",
            AiTool::Upscale { .. } => "AI Upscale",
            AiTool::Retouch { .. } => "Retouch portrait",
            AiTool::Cutout => "Remove background",
            AiTool::Remove => "Remove objects",
            AiTool::Mask { target } => match target {
                Target::Subject => "Select subject",
                Target::Sky => "Select sky",
                Target::People => "Select people",
                Target::Hair => "Select hair",
                Target::FaceSkin => "Select face skin",
                Target::BodySkin => "Select body skin",
                Target::Clothes => "Select clothes",
            },
            AiTool::Cull => "Assisted culling",
            AiTool::FindPeople => "Find people",
        }
    }

    /// Added to the photo's name, and the new file's extension.
    pub fn suffix(self) -> (String, &'static str) {
        match self {
            AiTool::Denoise { .. } => ("Denoised".into(), "tif"),
            AiTool::Upscale { factor } => (format!("Upscaled-{factor}x"), "tif"),
            AiTool::Retouch { .. } => ("Retouched".into(), "tif"),
            AiTool::Cutout => ("Cutout".into(), "png"),
            AiTool::Remove => ("Removed".into(), "tif"),
            AiTool::Mask { .. } | AiTool::Cull | AiTool::FindPeople => (String::new(), ""),
        }
    }
}

/// Where a run is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    /// Fetching the engine and models (bytes).
    Download { done: u64, total: u64 },
    /// Working on photo `index` (0-based) of `count`: `done` of `total` parts of it.
    Photo {
        index: usize,
        count: usize,
        done: usize,
        total: usize,
    },
}

/// What a run made and what it could not.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Report {
    pub written: Vec<PathBuf>,
    /// The photo's file name and why.
    pub failed: Vec<(String, String)>,
    /// AI masks found: the photo and the mask's shape.
    pub masks: Vec<(i64, lf_settings::masks::Shape)>,
    /// Assisted culling: what the photos' scores say.
    pub culled: Option<Culled>,
    /// Find people: what the scan found.
    pub people: Option<PeopleFound>,
}

/// What Find people found.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PeopleFound {
    /// Photos scanned by this run (the rest were scanned before).
    pub scanned: usize,
    /// Faces found in them.
    pub faces: usize,
    /// Named people and suggested groups in the catalogue now.
    pub named: usize,
    pub groups: usize,
}

/// What assisted culling found over the photos it was run on (scored now or before).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Culled {
    /// Photos scored by this run (the rest already had fresh scores).
    pub scored: usize,
    pub blurry: usize,
    pub eyes_closed: usize,
    /// Every photo that looks like a reject (blurry or eyes closed), in the order asked.
    pub issues: Vec<i64>,
}

/// The AI folder beside the frame cache (`<cache>/ai`).
pub fn ai_dir(frames_dir: &Path) -> PathBuf {
    frames_dir
        .parent()
        .map_or_else(|| frames_dir.join("ai"), |p| p.join("ai"))
}

/// The first free `<stem>-<suffix>[-n].<ext>` in `dir`.
pub fn free_name(dir: &Path, stem: &str, suffix: &str, ext: &str) -> PathBuf {
    (1..)
        .map(|n| {
            let name = if n == 1 {
                format!("{stem}-{suffix}.{ext}")
            } else {
                format!("{stem}-{suffix}-{n}.{ext}")
            };
            dir.join(name)
        })
        .find(|p| !p.exists())
        .expect("a free name")
}

/// Runs `tool` on `ids`. A photo that fails is reported and the others go on; only a
/// problem with the AI folder (or a stop) ends the run.
#[allow(clippy::too_many_arguments)]
pub fn run(
    gpu: Arc<Gpu>,
    engine: &Engine,
    root: &Path,
    ids: &[i64],
    tool: AiTool,
    painted: Option<&Gray>,
    mut progress: impl FnMut(Step),
    stop: &AtomicBool,
) -> Result<Report, String> {
    engine
        .prepare(
            &tool.models(),
            &mut |done, total| progress(Step::Download { done, total }),
            stop,
        )
        .map_err(|e| e.to_string())?;
    let catalog = Catalog::open(root).map_err(|e| e.to_string())?;
    if tool == AiTool::Cull {
        return cull(&gpu, engine, &catalog, ids, &mut progress, stop);
    }
    if tool == AiTool::FindPeople {
        return find_people(&gpu, engine, &catalog, ids, &mut progress, stop);
    }
    let mut report = Report::default();
    let count = ids.len();
    for (index, &id) in ids.iter().enumerate() {
        if stop.load(Ordering::SeqCst) {
            return Err(lf_ai::AiError::Stopped.to_string());
        }
        let Some(row) = catalog.photo(id).map_err(|e| e.to_string())? else {
            continue;
        };
        let name = crate::develop::open::file_name(&row.path);
        progress(Step::Photo {
            index,
            count,
            done: 0,
            total: 1,
        });
        let mut tick = |done, total| {
            progress(Step::Photo {
                index,
                count,
                done,
                total,
            });
        };
        let result = if let AiTool::Mask { target } = tool {
            mask(&gpu, engine, &catalog, id, &row.path, target).map(|shape| {
                report.masks.push((id, shape));
            })
        } else {
            one(
                &gpu, engine, &catalog, id, &row.path, tool, painted, &mut tick, stop,
            )
            .map(|path| {
                log::info!("{} wrote {}", tool.name(), path.display());
                report.written.push(path);
            })
        };
        match result {
            Ok(()) => {}
            Err(e) if stop.load(Ordering::SeqCst) => return Err(e),
            Err(e) => {
                log::warn!("{} failed on {name}: {e}", tool.name());
                report.failed.push((name, e));
            }
        }
    }
    Ok(report)
}

#[allow(clippy::too_many_arguments)]
fn one(
    gpu: &Arc<Gpu>,
    engine: &Engine,
    catalog: &Catalog,
    id: i64,
    path: &Path,
    tool: AiTool,
    painted: Option<&Gray>,
    progress: &mut dyn FnMut(usize, usize),
    stop: &AtomicBool,
) -> Result<PathBuf, String> {
    let settings = edits::get_settings(catalog, id)
        .map_err(|e| e.to_string())?
        .unwrap_or_else(EditSettings::default_settings);
    let mut img = render(gpu, path, &settings, gpu.max_edge())?;
    let dir = path
        .parent()
        .ok_or_else(|| "The photo's folder could not be found.".to_owned())?;
    let stem = path
        .file_stem()
        .map_or_else(|| "photo".to_owned(), |s| s.to_string_lossy().into_owned());
    let (suffix, ext) = tool.suffix();
    let out = free_name(dir, &stem, &suffix, ext);
    let text = |e: lf_ai::AiError| e.to_string();
    match tool {
        AiTool::Denoise { strength } => {
            let done =
                lf_ai::restore::denoise(engine, &img, strength, progress, stop).map_err(text)?;
            write_tiff16(&out, &done)?;
        }
        AiTool::Upscale { factor } => {
            // Larger than the limit: the photo is rendered smaller first, so the result fits.
            let mp = (img.w * factor) as f64 * (img.h * factor) as f64 / 1e6;
            if mp > lf_ai::restore::MAX_UPSCALED_MP {
                let k = (lf_ai::restore::MAX_UPSCALED_MP / mp).sqrt();
                img = img.resized(
                    ((img.w as f64 * k) as usize).max(1),
                    ((img.h as f64 * k) as usize).max(1),
                );
            }
            let done =
                lf_ai::restore::upscale(engine, &img, factor, progress, stop).map_err(text)?;
            write_tiff16(&out, &done)?;
        }
        AiTool::Retouch { amount } => {
            let faces = lf_ai::faces::detect(engine, &img).map_err(text)?;
            if faces.is_empty() {
                return Err("No face was found.".to_owned());
            }
            progress(1, 2);
            let done = lf_ai::faces::smooth_skin(&img, &faces, amount);
            write_tiff16(&out, &done)?;
        }
        AiTool::Mask { .. } | AiTool::Cull | AiTool::FindPeople => {
            return Err("Not a file.".to_owned());
        }
        AiTool::Remove => {
            let painted = painted.ok_or_else(|| "Nothing was painted.".to_owned())?;
            let mask = painted.resized(img.w, img.h);
            let done = lf_ai::inpaint::remove(engine, &img, &mask, progress, stop).map_err(text)?;
            write_tiff16(&out, &done)?;
        }
        AiTool::Cutout => {
            let long = img.w.max(img.h).max(1);
            let k = (1024.0 / long as f64).min(1.0);
            let small = img.resized(
                ((img.w as f64 * k) as usize).max(1),
                ((img.h as f64 * k) as usize).max(1),
            );
            let mask = lf_ai::segment::mask(
                engine,
                lf_ai::segment::Target::Subject,
                &small,
                img.w,
                img.h,
            )
            .map_err(text)?;
            progress(1, 2);
            write_cutout(&out, &img, &mask)?;
        }
    }
    progress(1, 1);
    Ok(out)
}

/// An AI mask of the subject (or the sky) of photo `id`: found on the photo as edited but
/// before its crop and turn (lens corrections kept: masks live in that space), at most
/// [`lf_settings::masks::MAX_IMAGE_EDGE`] on its long side.
fn mask(
    gpu: &Arc<Gpu>,
    engine: &Engine,
    catalog: &Catalog,
    id: i64,
    path: &Path,
    target: Target,
) -> Result<lf_settings::masks::Shape, String> {
    let mut settings = edits::get_settings(catalog, id)
        .map_err(|e| e.to_string())?
        .unwrap_or_else(EditSettings::default_settings);
    settings.rotate = lf_settings::Rotate::R0;
    settings.flip_h = false;
    settings.flip_v = false;
    settings.angle = 0.0;
    settings.crop = lf_settings::Crop {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
    };
    let img = render(gpu, path, &settings, 1024)?;
    let edge = f64::from(lf_settings::masks::MAX_IMAGE_EDGE);
    let k = (edge / img.w.max(img.h) as f64).min(1.0);
    let (w, h) = (
        ((img.w as f64 * k).round() as usize).max(1),
        ((img.h as f64 * k).round() as usize).max(1),
    );
    let found = lf_ai::segment::mask(engine, target, &img, w, h).map_err(|e| e.to_string())?;
    let values: Vec<u8> = found.to_luma8().into_raw();
    if values.iter().all(|&v| v < 13) {
        return Err(format!("No {} was found.", target.noun()));
    }
    Ok(lf_settings::masks::Shape::Image {
        what: target.key().to_owned(),
        width: w as u32,
        height: h as u32,
        data: lf_settings::masks::encode_coverage(&values),
    })
}

/// Assisted culling: scores the photos of `ids` that have no fresh scores (as shot, at
/// [`lf_ai::cull::SCORE_EDGE`]), then says what every photo's scores show. A photo that
/// cannot be read is reported and the others go on.
fn cull(
    gpu: &Arc<Gpu>,
    engine: &Engine,
    catalog: &Catalog,
    ids: &[i64],
    progress: &mut impl FnMut(Step),
    stop: &AtomicBool,
) -> Result<Report, String> {
    let mut scorer = lf_ai::cull::Scorer::new(engine).map_err(|e| e.to_string())?;
    let todo = catalog.unscored(ids).map_err(|e| e.to_string())?;
    let mut report = Report::default();
    let mut culled = Culled::default();
    let count = todo.len();
    let as_shot = EditSettings::default_settings();
    for (index, (id, path)) in todo.iter().enumerate() {
        if stop.load(Ordering::SeqCst) {
            return Err(lf_ai::AiError::Stopped.to_string());
        }
        progress(Step::Photo {
            index,
            count,
            done: 0,
            total: 1,
        });
        let scored = render_at(
            gpu,
            path,
            &as_shot,
            lf_ai::cull::SCORE_EDGE,
            Quality::Preview,
        )
        .and_then(|img| scorer.score(&img).map_err(|e| e.to_string()))
        .and_then(|s| {
            let scores = lf_catalog::scores::Scores {
                sharpness: s.sharpness,
                faces: s.faces,
                eyes_closed: s.eyes_closed,
                eye_sharpness: s.eye_sharpness,
            };
            catalog.set_scores(*id, &scores).map_err(|e| e.to_string())
        });
        match scored {
            Ok(()) => culled.scored += 1,
            Err(e) => {
                let name = crate::develop::open::file_name(path);
                log::warn!("assisted culling failed on {name}: {e}");
                report.failed.push((name, e));
            }
        }
    }
    for &id in ids {
        let Some(scores) = catalog
            .photo(id)
            .map_err(|e| e.to_string())?
            .and_then(|r| r.scores)
        else {
            continue;
        };
        culled.blurry += usize::from(scores.blurry());
        culled.eyes_closed += usize::from(scores.eyes_closed());
        if scores.issue() {
            culled.issues.push(id);
        }
    }
    report.culled = Some(culled);
    Ok(report)
}

/// Find people: scans the photos of `ids` not scanned as their files are now (as shot, at
/// [`lf_ai::identity::SCAN_EDGE`]) for faces, records them, and regroups every face of the
/// catalogue into people. A photo that cannot be read is reported and the others go on.
fn find_people(
    gpu: &Arc<Gpu>,
    engine: &Engine,
    catalog: &Catalog,
    ids: &[i64],
    progress: &mut impl FnMut(Step),
    stop: &AtomicBool,
) -> Result<Report, String> {
    let mut identifier = lf_ai::identity::Identifier::new(engine).map_err(|e| e.to_string())?;
    let todo = catalog
        .unscanned_for_faces(ids)
        .map_err(|e| e.to_string())?;
    let mut report = Report::default();
    let mut found = PeopleFound::default();
    let count = todo.len();
    let as_shot = EditSettings::default_settings();
    for (index, (id, path)) in todo.iter().enumerate() {
        if stop.load(Ordering::SeqCst) {
            return Err(lf_ai::AiError::Stopped.to_string());
        }
        progress(Step::Photo {
            index,
            count,
            done: 0,
            total: 1,
        });
        let scanned = render_at(
            gpu,
            path,
            &as_shot,
            lf_ai::identity::SCAN_EDGE,
            Quality::Preview,
        )
        .and_then(|img| {
            let faces = identifier.find(&img).map_err(|e| e.to_string())?;
            let (w, h) = (img.w as f32, img.h as f32);
            faces
                .into_iter()
                .map(|f| {
                    Ok(lf_catalog::people::NewFace {
                        rect: [f.face.x / w, f.face.y / h, f.face.w / w, f.face.h / h]
                            .map(|v| v.clamp(0.0, 1.0)),
                        description: f.description,
                        thumb: jpeg(&f.thumb)?,
                    })
                })
                .collect::<Result<Vec<_>, String>>()
        })
        .and_then(|faces| {
            catalog.set_faces(*id, &faces).map_err(|e| e.to_string())?;
            Ok(faces.len())
        });
        match scanned {
            Ok(n) => {
                found.scanned += 1;
                found.faces += n;
            }
            Err(e) => {
                let name = crate::develop::open::file_name(path);
                log::warn!("finding people failed on {name}: {e}");
                report.failed.push((name, e));
            }
        }
    }
    catalog.regroup_faces().map_err(|e| e.to_string())?;
    let list = catalog.people().map_err(|e| e.to_string())?;
    found.named = list.iter().filter(|p| p.name.is_some()).count();
    found.groups = list.len() - found.named;
    report.people = Some(found);
    Ok(report)
}

/// `img` as a JPEG (a face's picture for the People list).
fn jpeg(img: &Rgb) -> Result<Vec<u8>, String> {
    let rgb = image::DynamicImage::ImageRgba8(img.to_rgba8()).to_rgb8();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85)
        .write_image(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// The photo at `path` with `settings`, at full size (`max_edge` at most), as f32 RGB.
pub fn render(
    gpu: &Arc<Gpu>,
    path: &Path,
    settings: &EditSettings,
    max_edge: u32,
) -> Result<Rgb, String> {
    render_at(gpu, path, settings, max_edge, Quality::Best)
}

/// [`render`] with the RAW demosaiced at `quality`.
pub fn render_at(
    gpu: &Arc<Gpu>,
    path: &Path,
    settings: &EditSettings,
    max_edge: u32,
    quality: Quality,
) -> Result<Rgb, String> {
    let name = crate::develop::open::file_name(path);
    let develop = DevelopSettings::from_settings(settings);
    let opened = lf_raw::open::open(path, quality, &develop, max_edge)
        .map_err(|e| format!("{name}: {e}"))?;
    let mut renderer =
        lf_raw::render::renderer_for(gpu, &opened).map_err(|e| format!("{name}: {e}"))?;
    let f = renderer
        .render_f16(settings)
        .map_err(|e| format!("{name}: {e}"))?;
    let data = f
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [0, 1, 2].map(|c| p[c].to_f32().clamp(0.0, 1.0)))
        .collect();
    Ok(Rgb {
        w: f.width as usize,
        h: f.height as usize,
        data,
    })
}

/// Writes `img` as a 16-bit TIFF with the sRGB profile, through a temporary name.
pub fn write_tiff16(out: &Path, img: &Rgb) -> Result<(), String> {
    let pixels: Vec<u8> = img
        .data
        .iter()
        .flat_map(|v| ((v.clamp(0.0, 1.0) * 65535.0).round() as u16).to_ne_bytes())
        .collect();
    let mut bytes = Vec::new();
    {
        let mut encoder = image::codecs::tiff::TiffEncoder::new(std::io::Cursor::new(&mut bytes));
        let _ = encoder.set_icc_profile(lf_catalog::finish::srgb_icc());
        encoder
            .write_image(
                &pixels,
                img.w as u32,
                img.h as u32,
                image::ExtendedColorType::Rgb16,
            )
            .map_err(|e| format!("The new photo could not be encoded ({e})."))?;
    }
    write_atomic(out, &bytes)
}

/// Writes `img` with `mask` as its transparency, as an 8-bit PNG.
pub fn write_cutout(out: &Path, img: &Rgb, mask: &Gray) -> Result<(), String> {
    let mut rgba = img.to_rgba8();
    for (px, a) in rgba.pixels_mut().zip(&mask.data) {
        px.0[3] = (a.clamp(0.0, 1.0) * 255.0).round() as u8;
    }
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| format!("The cutout could not be encoded ({e})."))?;
    write_atomic(out, &bytes)
}

fn write_atomic(out: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = out.with_extension("lfai.tmp");
    std::fs::write(&tmp, bytes)
        .and_then(|()| std::fs::rename(&tmp, out))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("The new photo could not be written ({e}).")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_files_take_the_first_free_name() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            free_name(dir.path(), "DSC_1", "Denoised", "tif"),
            dir.path().join("DSC_1-Denoised.tif")
        );
        std::fs::write(dir.path().join("DSC_1-Denoised.tif"), b"x").unwrap();
        assert_eq!(
            free_name(dir.path(), "DSC_1", "Denoised", "tif"),
            dir.path().join("DSC_1-Denoised-2.tif")
        );
        assert_eq!(
            AiTool::Upscale { factor: 2 }.suffix(),
            ("Upscaled-2x".to_owned(), "tif")
        );
        assert_eq!(
            ai_dir(Path::new("C:/cache/frames")),
            Path::new("C:/cache/ai")
        );
    }

    #[test]
    fn a_cutout_keeps_the_mask_as_transparency_and_a_tiff_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let img = Rgb {
            w: 2,
            h: 1,
            data: vec![1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
        };
        let mask = Gray {
            w: 2,
            h: 1,
            data: vec![1.0, 0.0],
        };
        let png = dir.path().join("a.png");
        write_cutout(&png, &img, &mask).unwrap();
        let back = image::open(&png).unwrap().to_rgba8();
        assert_eq!(back.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(back.get_pixel(1, 0).0[3], 0);
        let tif = dir.path().join("a.tif");
        write_tiff16(&tif, &img).unwrap();
        assert!(matches!(
            image::open(&tif).unwrap(),
            image::DynamicImage::ImageRgb16(_)
        ));
    }

    #[test]
    fn a_photo_renders_with_its_edits_at_full_size() {
        let Some(gpu) = lf_pipeline::test_support::try_shared_gpu() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.png");
        image::RgbaImage::from_pixel(40, 30, image::Rgba([100, 100, 100, 255]))
            .save(&path)
            .unwrap();
        let mut s = EditSettings::default_settings();
        s.exposure = 1.0;
        let img = render(&gpu, &path, &s, 4096).unwrap();
        assert_eq!((img.w, img.h), (40, 30));
        assert!(img.at(5, 5)[0] > 100.0 / 255.0);
    }
}
