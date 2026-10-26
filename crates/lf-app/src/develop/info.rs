//! The info strip under the histogram: the file's name, size, camera, lens, exposure and
//! capture date, its kind, and whether the working copy was reduced.

use lf_raw::exif::ExifSummary;
use lf_raw::open::Kind;

/// The kind badge.
pub fn kind_badge(kind: Kind) -> &'static str {
    match kind {
        Kind::Raw => "RAW",
        Kind::Jpeg => "JPEG",
        Kind::Png => "PNG",
        Kind::Tiff => "TIFF",
        Kind::WebP => "WebP",
        Kind::Heic => "HEIC",
        Kind::Unknown => "Image",
    }
}

/// `6000 × 4000`.
pub fn dimensions(width: u32, height: u32) -> String {
    format!("{width} × {height}")
}

/// `1/250 s`, `0.3 s` is shown as `1/3 s`, and `2 s` or `1.5 s` for long exposures.
pub fn shutter(seconds: f32) -> Option<String> {
    if !(seconds.is_finite() && seconds > 0.0) {
        return None;
    }
    Some(if seconds < 1.0 {
        format!("1/{} s", (1.0 / seconds).round() as u32)
    } else if (seconds - seconds.round()).abs() < 0.05 {
        format!("{} s", seconds.round() as u32)
    } else {
        format!("{seconds:.1} s")
    })
}

/// `f/2.8`, `f/8`.
pub fn aperture(f_number: f32) -> Option<String> {
    (f_number.is_finite() && f_number > 0.0).then(|| format!("f/{}", trim(f_number, 1)))
}

/// `85 mm`, `4.5 mm`.
pub fn focal_length(mm: f32) -> Option<String> {
    (mm.is_finite() && mm > 0.0).then(|| format!("{} mm", trim(mm, 1)))
}

/// `ISO 400`.
pub fn iso(iso: u32) -> String {
    format!("ISO {iso}")
}

/// The camera: make and model, without the make twice (`Canon Canon EOS RP`).
pub fn camera(exif: &ExifSummary) -> Option<String> {
    match (exif.make.as_deref(), exif.model.as_deref()) {
        (Some(make), Some(model)) => {
            let first = make.split_whitespace().next().unwrap_or(make);
            if model.to_lowercase().starts_with(&first.to_lowercase()) {
                Some(model.to_owned())
            } else {
                Some(format!("{make} {model}"))
            }
        }
        (Some(only), None) | (None, Some(only)) => Some(only.to_owned()),
        (None, None) => None,
    }
}

/// `2024-05-01T12:34:56` as `2024-05-01 12:34`.
pub fn captured(at: &str) -> String {
    match at.split_once('T') {
        Some((date, time)) => format!("{date} {}", time.get(..5).unwrap_or(time)),
        None => at.to_owned(),
    }
}

/// The note shown when the working copy is smaller than the file.
pub fn reduced_note(edge: u32) -> String {
    format!("Reduced to {edge} px for editing")
}

/// The strip's lines, in order, for the facts the file has.
pub fn lines(exif: &ExifSummary) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(camera) = camera(exif) {
        out.push(camera);
    }
    if let Some(lens) = &exif.lens {
        out.push(lens.clone());
    }
    let exposure: Vec<String> = [
        exif.iso.map(iso),
        exif.exposure_time.and_then(shutter),
        exif.f_number.and_then(aperture),
        exif.focal_length.and_then(focal_length),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !exposure.is_empty() {
        out.push(exposure.join("  ·  "));
    }
    if let Some(at) = &exif.captured_at {
        out.push(captured(at));
    }
    out
}

/// `v` with at most `places` decimals and no trailing zeros.
fn trim(v: f32, places: usize) -> String {
    let text = format!("{v:.places$}");
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposure_facts_read_as_photographers_write_them() {
        assert_eq!(shutter(1.0 / 250.0).as_deref(), Some("1/250 s"));
        assert_eq!(shutter(0.3).as_deref(), Some("1/3 s"));
        assert_eq!(shutter(2.0).as_deref(), Some("2 s"));
        assert_eq!(shutter(1.5).as_deref(), Some("1.5 s"));
        assert_eq!(shutter(0.0), None);
        assert_eq!(aperture(2.8).as_deref(), Some("f/2.8"));
        assert_eq!(aperture(8.0).as_deref(), Some("f/8"));
        assert_eq!(focal_length(85.0).as_deref(), Some("85 mm"));
        assert_eq!(dimensions(6000, 4000), "6000 × 4000");
        assert_eq!(captured("2024-05-01T12:34:56"), "2024-05-01 12:34");
        assert_eq!(kind_badge(Kind::Raw), "RAW");
        assert_eq!(reduced_note(6144), "Reduced to 6144 px for editing");
    }

    #[test]
    fn camera_never_repeats_the_make() {
        let exif = |make: &str, model: &str| ExifSummary {
            make: Some(make.into()),
            model: Some(model.into()),
            ..Default::default()
        };
        assert_eq!(
            camera(&exif("Canon", "Canon EOS RP")).as_deref(),
            Some("Canon EOS RP")
        );
        assert_eq!(
            camera(&exif("NIKON CORPORATION", "NIKON D40")).as_deref(),
            Some("NIKON D40")
        );
        assert_eq!(
            camera(&exif("FUJIFILM", "X20")).as_deref(),
            Some("FUJIFILM X20")
        );
        let full = ExifSummary {
            iso: Some(400),
            exposure_time: Some(1.0 / 250.0),
            f_number: Some(2.8),
            focal_length: Some(85.0),
            ..exif("Sony", "ILCE-7S")
        };
        assert_eq!(
            lines(&full),
            ["Sony ILCE-7S", "ISO 400  ·  1/250 s  ·  f/2.8  ·  85 mm"]
        );
    }
}
