//! Photo → Merge to HDR, on the `Long` lane: each bracketed frame is decoded and developed
//! with the default settings (the photo as shot, RAW headroom and all, through the develop
//! pass), lined up with the middle frame ([`lf_merge::align`]), fused
//! ([`lf_merge::fuse`]) and written as a 16-bit TIFF with the sRGB profile beside the first
//! photo (`<name>-HDR.tif`, `-HDR-2`, …). The library then rescans its catalogue folder, so
//! the merged photo shows beside the frames, ready to edit.
//!
//! Frames are worked on at most [`MAX_EDGE`] pixels on the long side and kept as 16-bit
//! while waiting, so a seven-frame bracket of 45 MP RAW files still fits in memory.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use image::ImageEncoder as _;
use lf_catalog::Catalog;
use lf_merge::Rgb;
use lf_pipeline::Gpu;
use lf_raw::demosaic::Quality;
use lf_settings::{DevelopSettings, EditSettings};

/// The most frames a merge takes, and the long edge they are worked at.
pub const MAX_FRAMES: usize = 7;
pub const MAX_EDGE: u32 = 5000;

/// A frame held between the passes: 16-bit RGB.
struct Held {
    w: usize,
    h: usize,
    data: Vec<u16>,
    shift: (i32, i32),
}

impl Held {
    fn rgb(&self) -> Rgb {
        let img = Rgb {
            w: self.w,
            h: self.h,
            data: self.data.iter().map(|&v| f32::from(v) / 65535.0).collect(),
        };
        img.shifted(self.shift.0, self.shift.1)
    }
}

/// The merged photo's name for `stem` that is free in `dir`.
pub fn merged_stem(dir: &Path, stem: &str) -> String {
    let mut n = 1;
    loop {
        let candidate = if n == 1 {
            format!("{stem}-HDR")
        } else {
            format!("{stem}-HDR-{n}")
        };
        if !dir.join(format!("{candidate}.tif")).exists() {
            return candidate;
        }
        n += 1;
    }
}

/// Merges `ids` (in the order given); returns the merged file. `progress(done, total)` as
/// it goes; `stop` ends it between frames (nothing is written).
pub fn run(
    gpu: Arc<Gpu>,
    root: &Path,
    ids: &[i64],
    mut progress: impl FnMut(usize, usize),
    stop: &AtomicBool,
) -> Result<PathBuf, String> {
    if ids.len() < 2 {
        return Err(lf_merge::MergeError::TooFew.to_string());
    }
    if ids.len() > MAX_FRAMES {
        return Err(format!("A merge takes at most {MAX_FRAMES} photos."));
    }
    let catalog = Catalog::open(root).map_err(|e| e.to_string())?;
    // Steps: each frame, aligning, fusing, writing.
    let total = ids.len() + 3;
    progress(0, total);
    let edge = MAX_EDGE.min(gpu.max_edge());
    let settings = EditSettings::default_settings();
    let mut frames: Vec<Held> = Vec::with_capacity(ids.len());
    let mut first: Option<PathBuf> = None;
    for (i, &id) in ids.iter().enumerate() {
        if stop.load(Ordering::SeqCst) {
            return Err("The merge was stopped.".to_owned());
        }
        let row = catalog
            .photo(id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "A photo is no longer in the catalogue.".to_owned())?;
        let name = crate::develop::open::file_name(&row.path);
        let opened =
            lf_raw::open::open(&row.path, Quality::Best, &DevelopSettings::default(), edge)
                .map_err(|e| format!("{name}: {e}"))?;
        let mut renderer =
            lf_raw::render::renderer_for(&gpu, &opened).map_err(|e| format!("{name}: {e}"))?;
        let f = renderer
            .render_f16(&settings)
            .map_err(|e| format!("{name}: {e}"))?;
        let data: Vec<u16> = f
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| {
                [0, 1, 2].map(|c| (p[c].to_f32().clamp(0.0, 1.0) * 65535.0).round() as u16)
            })
            .collect();
        if let Some(prev) = frames.first()
            && (prev.w, prev.h) != (f.width as usize, f.height as usize)
        {
            return Err(format!(
                "{name} is not the same size as the first photo; a merge needs one bracket."
            ));
        }
        frames.push(Held {
            w: f.width as usize,
            h: f.height as usize,
            data,
            shift: (0, 0),
        });
        first.get_or_insert(row.path.clone());
        progress(i + 1, total);
    }
    // Line every frame up with the middle one (the most likely to be well exposed).
    let middle = frames.len() / 2;
    let reference = frames[middle].rgb();
    for (i, frame) in frames.iter_mut().enumerate() {
        if i != middle {
            frame.shift = lf_merge::align(&reference, &frame.rgb(), 6);
            log::info!("HDR frame {i} moved by {:?}", frame.shift);
        }
    }
    drop(reference);
    progress(ids.len() + 1, total);
    if stop.load(Ordering::SeqCst) {
        return Err("The merge was stopped.".to_owned());
    }
    let merged = lf_merge::fuse(frames.len(), |i| frames[i].rgb()).map_err(|e| e.to_string())?;
    drop(frames);
    progress(ids.len() + 2, total);

    let first = first.expect("two frames at least");
    let dir = first
        .parent()
        .ok_or_else(|| "The photo's folder could not be found.".to_owned())?;
    let stem = first
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "photo".to_owned());
    let out = dir.join(format!("{}.tif", merged_stem(dir, &stem)));
    // The encoder takes 16-bit samples as native-endian bytes.
    let pixels: Vec<u8> = merged
        .data
        .iter()
        .flat_map(|v| ((v * 65535.0).round() as u16).to_ne_bytes())
        .collect();
    let mut bytes = Vec::new();
    {
        let mut encoder = image::codecs::tiff::TiffEncoder::new(std::io::Cursor::new(&mut bytes));
        let _ = encoder.set_icc_profile(lf_catalog::finish::srgb_icc());
        encoder
            .write_image(
                &pixels,
                merged.w as u32,
                merged.h as u32,
                image::ExtendedColorType::Rgb16,
            )
            .map_err(|e| format!("The merged photo could not be encoded ({e})."))?;
    }
    let tmp = out.with_extension("tif.tmp");
    std::fs::write(&tmp, &bytes)
        .and_then(|()| std::fs::rename(&tmp, &out))
        .map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("The merged photo could not be written ({e}).")
        })?;
    progress(total, total);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_merged_photo_takes_the_first_free_name() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(merged_stem(dir.path(), "DSC_1"), "DSC_1-HDR");
        std::fs::write(dir.path().join("DSC_1-HDR.tif"), b"x").unwrap();
        assert_eq!(merged_stem(dir.path(), "DSC_1"), "DSC_1-HDR-2");
    }

    #[test]
    fn a_bracket_is_merged_into_a_16_bit_tiff_beside_the_first_frame() {
        let Some(gpu) = lf_pipeline::test_support::try_shared_gpu() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        // Three exposures of one scene: a dark half and a bright half with texture.
        for (i, gain) in [0.25f32, 1.0, 4.0].into_iter().enumerate() {
            let img = image::RgbaImage::from_fn(96, 64, |x, y| {
                let base = if x < 48 { 0.05 } else { 0.5 };
                let t = if (x / 4 + y / 4) % 2 == 0 { 1.2 } else { 0.8 };
                let v = ((base * t * gain).min(1.0) * 255.0) as u8;
                image::Rgba([v, v, v, 255])
            });
            img.save(dir.path().join(format!("b{i}.png"))).unwrap();
        }
        let catalog = Catalog::open(dir.path()).unwrap();
        let folder = catalog.add_folder(&dir.path().to_string_lossy()).unwrap();
        lf_catalog::import::run(
            &catalog,
            folder,
            &lf_catalog::import::scan(dir.path()).candidates,
            200,
            &mut |_| Ok(()),
        )
        .unwrap();
        let ids = catalog.photo_ids().unwrap();
        let mut steps = Vec::new();
        let out = run(
            gpu,
            dir.path(),
            &ids,
            |d, t| steps.push((d, t)),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(out, dir.path().join("b0-HDR.tif"));
        let merged = image::open(&out).unwrap();
        assert_eq!((merged.width(), merged.height()), (96, 64));
        assert!(matches!(merged, image::DynamicImage::ImageRgb16(_)));
        assert_eq!(steps.last(), Some(&(6, 6)));
        // The shadows come from the bright frame: lighter than the middle frame's.
        let px = |img: &image::DynamicImage, x, y| img.to_rgb16().get_pixel(x, y).0[0];
        let middle = image::open(dir.path().join("b1.png")).unwrap();
        assert!(px(&merged, 10, 30) > px(&middle, 10, 30));
    }

    #[test]
    fn one_photo_or_too_many_is_refused() {
        let Some(gpu) = lf_pipeline::test_support::try_shared_gpu() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let stop = AtomicBool::new(false);
        assert!(run(gpu.clone(), dir.path(), &[1], |_, _| {}, &stop).is_err());
        assert!(
            run(gpu, dir.path(), &[1; 8], |_, _| {}, &stop)
                .unwrap_err()
                .contains("at most")
        );
    }
}
