//! "Match a Lightroom example…": the job. The user gives an original photo and Lightroom's
//! export of it with a preset applied; we render the original our own way (the preset's
//! sliders, or the defaults, with no table, vignette or grain), learn the colour table,
//! vignette and grain that turn ours into theirs ([`lf_grade::example::fit`]), and build the
//! preset that carries them, ready to save.
//!
//! The fit's renders are the real pipeline, not a CPU copy of its maths: each candidate table
//! is registered under its embedded name ([`lf_settings::lut::embedded_name`]) and the
//! original is rendered again with it, so what the fit measures is what applying the preset
//! will show. The photo is opened once and its renderer reused for every render.
//!
//! Lightroom's export may be large (a 24 MP frame is ~288 MB as f32), so it is shrunk to
//! about twice the working size while still 8 or 16-bit, then converted from its embedded
//! ICC profile to sRGB (an export without one is taken as sRGB). Runs on the `Long` lane.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;
use lf_grade::example::{self, Fit, Image, TARGET_MEAN, TARGET_P95};
use lf_pipeline::Gpu;
use lf_settings::lightroom::LIGHTROOM_KEY;
use lf_settings::lightroom::budget::fit_budget;
use lf_settings::lut::{LutChoice, decode_table, embedded_name, encode_table};
use lf_settings::{DevelopSettings, EditSettings};
use serde_json::{Value, json};

/// The long edge our render is fitted at.
pub const WORKING_EDGE: u32 = 1024;
/// The long edge Lightroom's export is shrunk to before it becomes f32: twice the working
/// size, so the fit's own area-averaging resize still has detail to average.
pub const EXPORT_EDGE: u32 = 2 * WORKING_EDGE;
/// The long edge of the dialog's four previews.
pub const PREVIEW_EDGE: usize = 512;
/// How much the difference preview is brightened, so small differences show.
pub const DIFFERENCE_GAIN: f32 = 4.0;

/// Shown when the export's embedded profile cannot be turned into sRGB.
pub const NOT_CONVERTED: &str = "Export from Lightroom in sRGB, or with an embedded profile.";
/// Shown when a candidate table is missing from the registry after a render (it could not be
/// made, or went missing): the render is not used.
pub const TABLE_LOST: &str = "The colour table could not be kept while rendering. Try again.";
/// Shown after Stop.
pub const STOPPED: &str = "The match was stopped.";
/// The verdicts ([`Verdict::text`]).
pub const VERY_CLOSE: &str = "Very close to Lightroom's export.";
pub const CLOSE: &str = "Close to Lightroom's export.";
pub const NOT_A_GOOD_MATCH: &str =
    "Not a good match: was the export cropped, straightened or lens-corrected?";

/// The export could not be read: the decoder's words.
pub fn unreadable_text(e: impl std::fmt::Display) -> String {
    format!("Lightroom's export could not be read ({e}).")
}

/// The four pictures the dialog shows, each at most [`PREVIEW_EDGE`] on its long edge.
#[derive(Clone, Debug, PartialEq)]
pub struct Previews {
    /// Our render before the match (no table, vignette or grain).
    pub before: egui::ColorImage,
    /// Lightroom's export, at our size.
    pub lightroom: egui::ColorImage,
    /// Our render with the matched preset.
    pub after: egui::ColorImage,
    /// `|after - lightroom|`, brightened [`DIFFERENCE_GAIN`] times.
    pub difference: egui::ColorImage,
}

/// How good a match is, in plain words. Judged on lining up *and* the difference: a pair can
/// line up while a lens profile or a straightening on one side keeps the difference high.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    VeryClose,
    Close,
    NotAGoodMatch,
}

impl Verdict {
    pub fn of(fit: &Fit) -> Verdict {
        if fit.aligned && fit.delta_e_mean < TARGET_MEAN && fit.delta_e_p95 < TARGET_P95 {
            Verdict::VeryClose
        } else if fit.aligned && fit.delta_e_mean < 5.0 {
            Verdict::Close
        } else {
            Verdict::NotAGoodMatch
        }
    }

    pub fn text(self) -> &'static str {
        match self {
            Verdict::VeryClose => VERY_CLOSE,
            Verdict::Close => CLOSE,
            Verdict::NotAGoodMatch => NOT_A_GOOD_MATCH,
        }
    }
}

/// The settings our base render uses: the preset laid on the defaults as applying it would
/// (or the defaults alone), without its table, vignette and grain, which are what the fit
/// learns.
pub fn base_settings(preset: Option<&Value>) -> EditSettings {
    let defaults = EditSettings::default_settings();
    let mut s = match preset {
        Some(p) => {
            let mut p = p.clone();
            if let Some(d) = p.get_mut("desktop").and_then(Value::as_object_mut) {
                d.remove("lut");
            }
            lf_settings::looks::apply_look(&defaults, &p)
        }
        None => defaults,
    };
    LutChoice::write_into(None, &mut s);
    s.vignette = 0.0;
    s.grain = 0.0;
    s
}

/// `base` with the fit's table (registered as `lut_name`) at full amount, vignette and grain.
fn effects_settings(base: &EditSettings, lut_name: &str, fit: &Fit) -> EditSettings {
    let mut s = base.clone();
    let choice = LutChoice {
        name: lut_name.to_owned(),
        amount: 100.0,
    };
    LutChoice::write_into(Some(&choice), &mut s);
    s.vignette = fit.vignette;
    s.vignette_midpoint = fit.vignette_midpoint;
    s.grain = fit.grain;
    s.grain_size = fit.grain_size;
    s
}

/// The new preset: the matched one's settings (when one is matched) with the fit's table
/// (`desktop.lut`, at amount 100), vignette and grain, and the difference reached
/// (`desktop.lightroom.example`). `desktop.lightroom` is made (named `name`) when missing, so
/// the preset applies Lightroom's way: only what it carries. Kept under the size cap
/// ([`fit_budget`]); the caller checks [`lf_settings::lightroom::budget::desktop_fits`]
/// before saving.
pub fn matched_preset(preset: Option<&Value>, name: &str, fit: &Fit) -> Value {
    let mut out = match preset {
        Some(p) if p.is_object() => p.clone(),
        _ => json!({ "v": lf_settings::SETTINGS_VERSION }),
    };
    let round2 = |v: f64| (v * 100.0).round() / 100.0;
    let o = out.as_object_mut().expect("an object");
    o.insert("vignette".into(), json!(fit.vignette));
    o.insert("vignetteMidpoint".into(), json!(fit.vignette_midpoint));
    // Grain is written even at 0: the table and vignette were fitted with exactly this grain,
    // and a photo's own grain left in place would add to Lightroom's look (the Effects group
    // is carried anyway for the vignette, so an absent grain would mean 0 all the same).
    o.insert("grain".into(), json!(fit.grain));
    o.insert("grainSize".into(), json!(fit.grain_size));
    if !o.get("desktop").is_some_and(Value::is_object) {
        o.insert("desktop".into(), json!({}));
    }
    let desktop = o["desktop"].as_object_mut().expect("an object");
    let mut lut = encode_table(&fit.table);
    lut["amount"] = json!(100);
    desktop.insert("lut".into(), lut);
    let lightroom = desktop
        .entry(LIGHTROOM_KEY)
        .or_insert_with(|| json!({ "name": name }));
    if !lightroom.is_object() {
        *lightroom = json!({ "name": name });
    }
    lightroom["example"] = json!({
        "deltaEMean": round2(fit.delta_e_mean),
        "deltaEP95": round2(fit.delta_e_p95),
    });
    if let Some(note) = fit_budget(&mut out) {
        log::info!("matched preset {name}: {note}");
    }
    out
}

/// The match, with our pipeline injected: `render` renders the original with the settings
/// given (always at the same size). `theirs` is Lightroom's export (any size). `name` names
/// `desktop.lightroom` when the preset had none.
pub fn match_with(
    render: &mut dyn FnMut(&EditSettings) -> Result<Image, String>,
    theirs: &Image,
    preset: Option<&Value>,
    name: &str,
    stop: &AtomicBool,
) -> Result<(Value, Fit, Previews), String> {
    let stopped = || stop.load(Ordering::Relaxed);
    let base = base_settings(preset);
    let ours = render(&base)?;
    if stopped() {
        return Err(STOPPED.to_owned());
    }
    if ours.w * ours.h == 0 || theirs.w * theirs.h == 0 {
        return Err(unreadable_text("no pixels"));
    }
    let mut failed: Option<String> = None;
    let mut last: Option<Image> = None;
    // The table as the preset will carry it (16-bit) and its embedded name, made once per
    // table the fit tries.
    // Each is pinned while the fit renders with it, so a catalogue opening meanwhile (which
    // clears the registry) cannot make a render go without it.
    let mut candidate: Option<(Vec<[f32; 3]>, String, Option<lf_pipeline::lut::Pinned>)> = None;
    let fit = example::fit(&ours, theirs, &mut |f: &Fit| {
        if !candidate
            .as_ref()
            .is_some_and(|(data, ..)| *data == f.table.data)
        {
            let encoded = encode_table(&f.table);
            let name = lf_settings::lut::well_formed_hash(&encoded)
                .map(embedded_name)
                .unwrap_or_default();
            let pinned = decode_table(&encoded).map(|t| lf_pipeline::lut::pin(&name, t));
            // The table before it is no longer needed: off the registry unless it was there.
            if let Some((.., Some(old))) = candidate.take() {
                old.forget();
            }
            candidate = Some((f.table.data.clone(), name, pinned));
        }
        let Some((_, name, _)) = &candidate else {
            unreachable!("a candidate was just made")
        };
        let img = if failed.is_some() || stopped() {
            ours.clone()
        } else {
            match render_with_table(render, &effects_settings(&base, name, f), name) {
                Ok(img) => img,
                Err(e) => {
                    failed = Some(e);
                    ours.clone()
                }
            }
        };
        last = Some(img.clone());
        img
    });
    // The last candidate too: whether the preset is saved or not, the fit's tables must not
    // stay on offer without a `.cube` file (saving writes the file and registers it again).
    if let Some((.., Some(last))) = candidate.take() {
        last.forget();
    }
    if let Some(e) = failed {
        return Err(e);
    }
    if stopped() {
        return Err(STOPPED.to_owned());
    }
    log::info!(
        "Lightroom example fit: residual {:.4}, dE mean {:.2}, p95 {:.2}, aligned {}",
        fit.residual,
        fit.delta_e_mean,
        fit.delta_e_p95,
        fit.aligned
    );
    let lightroom = example::resize(theirs, ours.w, ours.h);
    let after = last.unwrap_or_else(|| ours.clone());
    let difference = Image {
        w: ours.w,
        h: ours.h,
        data: after
            .data
            .iter()
            .zip(&lightroom.data)
            .map(|(a, b)| ((a - b).abs() * DIFFERENCE_GAIN).min(1.0))
            .collect(),
    };
    let previews = Previews {
        before: preview(&ours),
        lightroom: preview(&lightroom),
        after: preview(&after),
        difference: preview(&difference),
    };
    Ok((matched_preset(preset, name, &fit), fit, previews))
}

/// `render(settings)`, where `settings` names the table registered (and pinned) as `name`;
/// an error when the table is not there after all, never a render without it to fit on.
fn render_with_table(
    render: &mut dyn FnMut(&EditSettings) -> Result<Image, String>,
    settings: &EditSettings,
    name: &str,
) -> Result<Image, String> {
    let img = render(settings)?;
    if lf_pipeline::lut::get(name).is_none() {
        return Err(TABLE_LOST.to_owned());
    }
    Ok(img)
}

/// `img` shrunk to [`PREVIEW_EDGE`] (never enlarged), as an egui picture.
fn preview(img: &Image) -> egui::ColorImage {
    let long = img.w.max(img.h);
    let small = if long > PREVIEW_EDGE {
        let k = PREVIEW_EDGE as f64 / long as f64;
        let w = ((img.w as f64 * k).round() as usize).max(1);
        let h = ((img.h as f64 * k).round() as usize).max(1);
        example::resize(img, w, h)
    } else {
        img.clone()
    };
    let bytes: Vec<u8> = small
        .data
        .iter()
        .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect();
    egui::ColorImage::from_rgb([small.w, small.h], &bytes)
}

/// Lightroom's export at `path`: decoded, turned upright, shrunk to [`EXPORT_EDGE`] at most,
/// and converted from its embedded profile to sRGB (none: taken as sRGB).
pub fn read_export(path: &Path) -> Result<Image, String> {
    use image::ImageDecoder as _;
    let reader = image::ImageReader::open(path)
        .map_err(unreadable_text)?
        .with_guessed_format()
        .map_err(unreadable_text)?;
    let mut decoder = reader.into_decoder().map_err(unreadable_text)?;
    let icc = decoder.icc_profile().map_err(unreadable_text)?;
    let orientation = decoder.orientation().map_err(unreadable_text)?;
    let mut img = image::DynamicImage::from_decoder(decoder).map_err(unreadable_text)?;
    img.apply_orientation(orientation);
    // 16-bit before shrinking: the resize writes the format it is given, and averaging into
    // 8 bits would round the export again (fine detail the grain fit would count).
    let mut img = image::DynamicImage::ImageRgb16(img.into_rgb16());
    let edge = EXPORT_EDGE;
    if img.width().max(img.height()) > edge {
        img = img.resize(edge, edge, image::imageops::FilterType::Triangle);
    }
    let rgb = img.into_rgb16();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let mut pixels = rgb.into_raw();
    if let Some(icc) = icc.filter(|p| !p.is_empty()) {
        lf_catalog::colour::to_srgb16(&icc, &mut pixels).map_err(|e| {
            log::warn!("Lightroom's export {}: {e}", path.display());
            NOT_CONVERTED.to_owned()
        })?;
    }
    Ok(Image {
        w,
        h,
        data: pixels.iter().map(|&v| f32::from(v) / 65535.0).collect(),
    })
}

/// The whole job: reads Lightroom's `export`, opens the `original` once (its own renderer on
/// the shared device), and matches; `preset` is the preset being matched, if any, and `name`
/// its name (or the new preset's). Gives the new preset, the fit and the previews; an error
/// is ready to show.
pub fn run(
    gpu: &Arc<Gpu>,
    original: &Path,
    export: &Path,
    preset: Option<&Value>,
    name: &str,
    stop: &AtomicBool,
) -> Result<(Value, Fit, Previews), String> {
    let theirs = read_export(export)?;
    if stop.load(Ordering::Relaxed) {
        return Err(STOPPED.to_owned());
    }
    let file = crate::develop::open::file_name(original);
    let develop = DevelopSettings::from_settings(&base_settings(preset));
    let opened = lf_raw::open::open(
        original,
        lf_raw::demosaic::Quality::Best,
        &develop,
        WORKING_EDGE,
    )
    .map_err(|e| format!("{file}: {e}"))?;
    // An imported preset's Auto WB / Auto tone, from this photo, as applying it would.
    let (wb, tone) = preset.map_or((false, false), lf_settings::lightroom::auto_flags);
    let autos = (wb || tone).then(|| crate::develop::auto::Sample::of(&opened.picture));
    let mut renderer =
        lf_raw::render::renderer_for(gpu, &opened).map_err(|e| format!("{file}: {e}"))?;
    drop(opened);
    let mut render = |s: &EditSettings| -> Result<Image, String> {
        let mut s = s.clone();
        if let Some(sample) = autos.as_ref() {
            // A skipped auto leaves the preset's own values, as applying it does.
            let _ = crate::develop::auto::apply_auto(&mut s, sample, wb, tone);
        }
        let f = renderer
            .render_f16(&s)
            .map_err(|e| format!("{file}: {e}"))?;
        let data = f
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [0, 1, 2].map(|c| p[c].to_f32().clamp(0.0, 1.0)))
            .collect();
        Ok(Image {
            w: f.width as usize,
            h: f.height as usize,
            data,
        })
    };
    match_with(&mut render, &theirs, preset, name, stop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lf_grade::example::{radius, vignette};
    use lf_settings::lightroom::budget::desktop_fits;
    use lf_settings::lut::Lut;

    const W: usize = 256;
    const H: usize = 192;

    /// A colourful picture covering the colour cube (as `lf-grade`'s example tests): 4 x 4
    /// tiles inside a grey frame, every channel kept just inside 0..1.
    fn colourful() -> Image {
        const FRAME: usize = 16;
        let (tw, th) = ((W - 2 * FRAME) / 4, (H - 2 * FRAME) / 4);
        let level = |i: usize, n: usize| 0.01 + 0.98 * i as f32 / (n - 1) as f32;
        let mut data = Vec::with_capacity(W * H * 3);
        for y in 0..H {
            for x in 0..W {
                let inside = (FRAME..W - FRAME).contains(&x) && (FRAME..H - FRAME).contains(&y);
                if !inside {
                    data.extend_from_slice(&[0.5; 3]);
                    continue;
                }
                let (lx, ly) = (x - FRAME, y - FRAME);
                let tile = (ly / th) * 4 + lx / tw;
                data.extend_from_slice(&[level(lx % tw, tw), level(ly % th, th), level(tile, 16)]);
            }
        }
        Image { w: W, h: H, data }
    }

    /// Lightroom's look: an S-curve on each channel, red lifted, blue lowered.
    fn look(c: [f32; 3]) -> [f32; 3] {
        let s = |x: f32| x - 0.08 * (2.0 * std::f32::consts::PI * x).sin();
        [0.02 + 0.96 * s(c[0]), s(c[1]), 0.92 * s(c[2])]
    }

    fn map(img: &Image, f: impl Fn(usize, usize, [f32; 3]) -> [f32; 3]) -> Image {
        let mut data = Vec::with_capacity(img.data.len());
        for y in 0..img.h {
            for x in 0..img.w {
                let i = (y * img.w + x) * 3;
                data.extend_from_slice(&f(x, y, [img.data[i], img.data[i + 1], img.data[i + 2]]));
            }
        }
        Image {
            w: img.w,
            h: img.h,
            data,
        }
    }

    /// The injected pipeline: the photo with the registered table the settings name, their
    /// vignette and a deterministic grain, recording each settings it was given.
    fn pipeline<'a>(
        photo: &'a Image,
        seen: &'a mut Vec<EditSettings>,
    ) -> impl FnMut(&EditSettings) -> Result<Image, String> + 'a {
        move |s: &EditSettings| {
            seen.push(s.clone());
            let table = LutChoice::from_settings(s).and_then(|c| lf_pipeline::lut::get(&c.name));
            Ok(map(photo, |x, y, c| {
                let c = table.as_ref().map_or(c, |t| t.apply(c));
                let c = vignette(c, radius(x, y, W, H), s.vignette, s.vignette_midpoint);
                let n = noise(x, y) * (s.grain / 100.0) as f32 * 0.2;
                c.map(|v| (v + n).clamp(0.0, 1.0))
            }))
        }
    }

    /// Repeatable noise in -0.5..0.5, the same for every render (as the shader's).
    fn noise(x: usize, y: usize) -> f32 {
        let mut h = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA77);
        h ^= h >> 15;
        h = h.wrapping_mul(0x2C1B_3C6D);
        h ^= h >> 12;
        (h & 0xFFFF) as f32 / 65535.0 - 0.5
    }

    /// `img` as an 8-bit file stores it.
    fn eight_bit(img: &Image) -> Image {
        map(img, |_, _, c| c.map(|v| (v * 255.0).round() / 255.0))
    }

    #[test]
    fn a_grain_free_8_bit_export_gets_no_grain() {
        let _turn = turn();
        let photo = eight_bit(&colourful());
        let theirs = eight_bit(&map(&photo, |_, _, c| look(c)));
        let mut seen = Vec::new();
        let (out, fit, _) = match_with(
            &mut pipeline(&photo, &mut seen),
            &theirs,
            None,
            "x",
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(fit.grain < 5.0, "grain {}", fit.grain);
        assert_eq!(out["grain"].as_f64(), Some(fit.grain));
    }

    #[test]
    fn a_render_without_its_table_is_an_error() {
        let _turn = turn();
        let mut render = |_: &EditSettings| -> Result<Image, String> {
            Ok(Image {
                w: 1,
                h: 1,
                data: vec![0.0; 3],
            })
        };
        let s = EditSettings::default_settings();
        assert_eq!(
            render_with_table(&mut render, &s, "lr-never-registered").unwrap_err(),
            TABLE_LOST
        );
    }

    /// The LUT registry is process-wide: tests that use it take turns with `luts`'s.
    fn turn() -> std::sync::MutexGuard<'static, ()> {
        crate::luts::REGISTRY_TURN
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    fn lightroom_export(photo: &Image) -> Image {
        map(photo, |x, y, c| {
            vignette(look(c), radius(x, y, W, H), -30.0, 50.0)
        })
    }

    #[test]
    fn the_preset_gets_its_table_and_effects() {
        let _turn = turn();
        let photo = colourful();
        let theirs = lightroom_export(&photo);
        let preset = json!({
            "v": 2,
            "exposure": 0.5,
            "vignette": 40,
            "desktop": {"lightroom": {"name": "Moody", "settings": {"crs:Exposure2012": "+0.50"}}},
        });
        let mut seen = Vec::new();
        let stop = AtomicBool::new(false);
        let (out, fit, previews) = match_with(
            &mut pipeline(&photo, &mut seen),
            &theirs,
            Some(&preset),
            "Studio - Moody",
            &stop,
        )
        .unwrap();

        // Our base render: the preset's sliders, without table, vignette or grain.
        let base = &seen[0];
        assert_eq!(base.exposure, 0.5);
        assert_eq!((base.vignette, base.grain), (0.0, 0.0));
        assert!(LutChoice::from_settings(base).is_none());
        // The fit's renders carry its table, registered under the embedded name.
        let last = seen.last().unwrap();
        let choice = LutChoice::from_settings(last).unwrap();
        assert!(choice.name.starts_with("lr-"), "{}", choice.name);
        assert_eq!(choice.amount, 100.0);

        // The preset: its own settings, the table, the effects and the difference reached.
        assert_eq!(out["exposure"], json!(0.5));
        let lut = &out["desktop"]["lut"];
        assert_eq!(lut["amount"], json!(100));
        let table = decode_table(lut).expect("a table the preset carries");
        assert_eq!(table.size, example::TABLE_SIZE);
        assert_eq!(embedded_name(lut["hash"].as_str().unwrap()), choice.name);
        let v = out["vignette"].as_f64().unwrap();
        assert!((v + 30.0).abs() <= 5.0, "vignette {v}");
        assert!(out["vignetteMidpoint"].is_number() && out["grain"].is_number());
        assert!(out["grainSize"].is_number());
        let lr = &out["desktop"]["lightroom"];
        assert_eq!(lr["name"], json!("Moody"), "the preset's own name stays");
        assert_eq!(lr["settings"]["crs:Exposure2012"], json!("+0.50"));
        let mean = lr["example"]["deltaEMean"].as_f64().unwrap();
        assert!(mean < TARGET_MEAN, "{mean}");
        assert!(lr["example"]["deltaEP95"].is_number());
        assert!(desktop_fits(&out));
        assert_eq!(Verdict::of(&fit), Verdict::VeryClose);

        // Four previews at our size (never enlarged).
        for p in [
            &previews.before,
            &previews.lightroom,
            &previews.after,
            &previews.difference,
        ] {
            assert_eq!(p.size, [W, H]);
        }
    }

    #[test]
    fn the_fits_tables_are_not_left_registered() {
        let _turn = turn();
        lf_pipeline::lut::clear();
        let photo = colourful();
        let theirs = lightroom_export(&photo);
        let mut seen = Vec::new();
        match_with(
            &mut pipeline(&photo, &mut seen),
            &theirs,
            None,
            "x",
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(
            lf_pipeline::lut::names().iter().all(|n| !n.starts_with("lr-")),
            "{:?}",
            lf_pipeline::lut::names()
        );
    }

    #[test]
    fn a_new_preset_applies_lightroom_style() {
        let _turn = turn();
        let photo = colourful();
        let theirs = lightroom_export(&photo);
        let mut seen = Vec::new();
        let (out, _, _) = match_with(
            &mut pipeline(&photo, &mut seen),
            &theirs,
            None,
            "portrait-export",
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(seen[0], base_settings(None));
        assert!(lf_settings::lightroom::is_lightroom(&out));
        assert_eq!(
            out["desktop"]["lightroom"]["name"],
            json!("portrait-export")
        );
        // Applied to a photo, it lays on its table and vignette and keeps the rest.
        let mut photo_settings = EditSettings::default_settings();
        photo_settings.exposure = 1.0;
        let applied =
            lf_settings::looks::apply_look(&photo_settings, &lf_settings::lut::name_embedded(&out));
        assert_eq!(applied.exposure, 1.0);
        assert!(LutChoice::from_settings(&applied).is_some());
        assert!(applied.vignette < -20.0, "{}", applied.vignette);
    }

    #[test]
    fn a_cropped_export_is_reported_not_saved_silently() {
        let _turn = turn();
        let photo = colourful();
        let full = lightroom_export(&photo);
        let (cw, ch) = (W * 8 / 10, H * 8 / 10);
        let (ox, oy) = ((W - cw) / 2, (H - ch) / 2);
        let mut data = Vec::with_capacity(cw * ch * 3);
        for y in 0..ch {
            let i = ((y + oy) * W + ox) * 3;
            data.extend_from_slice(&full.data[i..i + cw * 3]);
        }
        let cropped = Image { w: cw, h: ch, data };
        let mut seen = Vec::new();
        let (out, fit, _) = match_with(
            &mut pipeline(&photo, &mut seen),
            &cropped,
            None,
            "cropped",
            &AtomicBool::new(false),
        )
        .unwrap();
        let verdict = Verdict::of(&fit);
        assert_eq!(verdict, Verdict::NotAGoodMatch, "{fit:?}");
        assert!(
            verdict.text().to_lowercase().contains("not a good match"),
            "{}",
            verdict.text()
        );
        // The preset is still offered (the verdict warns; saving stays the user's call).
        assert!(out.pointer("/desktop/lut/table").is_some());
    }

    fn fit_with(aligned: bool, mean: f64, p95: f64) -> Fit {
        Fit {
            table: Lut::identity(2),
            vignette: 0.0,
            vignette_midpoint: 50.0,
            grain: 0.0,
            grain_size: 25.0,
            delta_e_mean: mean,
            delta_e_p95: p95,
            aligned,
            residual: 0.0,
        }
    }

    #[test]
    fn the_verdict_needs_lining_up_and_a_small_difference() {
        assert_eq!(Verdict::of(&fit_with(true, 1.6, 3.9)), Verdict::VeryClose);
        assert_eq!(Verdict::of(&fit_with(true, 1.6, 6.0)), Verdict::Close);
        assert_eq!(Verdict::of(&fit_with(true, 4.9, 9.0)), Verdict::Close);
        assert_eq!(
            Verdict::of(&fit_with(true, 5.0, 9.0)),
            Verdict::NotAGoodMatch
        );
        assert_eq!(
            Verdict::of(&fit_with(false, 0.5, 1.0)),
            Verdict::NotAGoodMatch
        );
    }

    #[test]
    fn a_pinned_table_survives_a_catalogue_opening_mid_render() {
        let _turn = turn();
        let _pinned = lf_pipeline::lut::pin("lr-pinned", Lut::identity(2));
        let mut render = |_: &EditSettings| -> Result<Image, String> {
            // A catalogue opened meanwhile.
            lf_pipeline::lut::clear();
            let had = lf_pipeline::lut::get("lr-pinned").is_some();
            Ok(Image {
                w: 1,
                h: 1,
                data: vec![if had { 1.0 } else { 0.0 }; 3],
            })
        };
        let s = EditSettings::default_settings();
        let img = render_with_table(&mut render, &s, "lr-pinned").unwrap();
        assert_eq!(img.data, [1.0; 3], "the render had its table");
    }

    #[test]
    fn stop_ends_the_match() {
        let _turn = turn();
        let photo = colourful();
        let mut seen = Vec::new();
        let result = match_with(
            &mut pipeline(&photo, &mut seen),
            &photo,
            None,
            "x",
            &AtomicBool::new(true),
        );
        assert_eq!(result.unwrap_err(), STOPPED);
        assert_eq!(seen.len(), 1, "nothing rendered after the base");
    }

    /// `img` as 16-bit values.
    fn sixteen(img: &Image) -> Vec<u16> {
        img.data
            .iter()
            .map(|v| (v * 65535.0).round() as u16)
            .collect()
    }

    fn write_png(path: &Path, w: usize, h: usize, pixels: Vec<u16>, icc: Option<Vec<u8>>) {
        use image::ImageEncoder as _;
        let file = std::fs::File::create(path).unwrap();
        let mut encoder = image::codecs::png::PngEncoder::new(file);
        if let Some(icc) = icc {
            encoder.set_icc_profile(icc).unwrap();
        }
        let buf =
            image::ImageBuffer::<image::Rgb<u16>, _>::from_raw(w as u32, h as u32, pixels).unwrap();
        image::DynamicImage::ImageRgb16(buf)
            .write_with_encoder(encoder)
            .unwrap();
    }

    fn max_difference(a: &Image, b: &Image) -> f32 {
        a.data
            .iter()
            .zip(&b.data)
            .map(|(x, y)| (x - y).abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn an_adobe_rgb_export_is_converted() {
        let _turn = turn();
        let dir = tempfile::tempdir().unwrap();
        let photo = colourful();
        // Lightroom's export in Adobe RGB, with its profile.
        let mut adobe = sixteen(&photo);
        let icc =
            lf_catalog::colour::convert16(&mut adobe, &lf_catalog::colour::OutputSpace::AdobeRgb)
                .unwrap();
        let tagged = dir.path().join("export-adobe.png");
        write_png(&tagged, W, H, adobe.clone(), Some(icc));
        let read = read_export(&tagged).unwrap();
        assert_eq!((read.w, read.h), (W, H));
        // Back in sRGB, within the rounding of the export's own (optimised) conversion.
        let d = max_difference(&read, &photo);
        assert!(d < 3.0 / 255.0, "converted back to sRGB: {d}");
        // Untagged, the same numbers are taken as sRGB, so they differ.
        let untagged = dir.path().join("export-untagged.png");
        write_png(&untagged, W, H, adobe, None);
        assert!(max_difference(&read_export(&untagged).unwrap(), &photo) > 0.05);
        // The fit sees it as it would an sRGB export of the same colours.
        let mut seen = Vec::new();
        let (_, fit, _) = match_with(
            &mut pipeline(&photo, &mut seen),
            &read,
            None,
            "x",
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(fit.delta_e_mean < 0.5, "{fit:?}");
    }

    #[test]
    fn a_profile_that_cannot_be_used_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("export.png");
        write_png(
            &path,
            4,
            4,
            vec![30000; 48],
            Some(b"not a profile".to_vec()),
        );
        assert_eq!(read_export(&path).unwrap_err(), NOT_CONVERTED);
        assert!(
            read_export(&dir.path().join("missing.jpg"))
                .unwrap_err()
                .starts_with("Lightroom's export could not be read")
        );
    }

    #[test]
    fn a_large_export_is_shrunk_before_it_becomes_float() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.png");
        let (w, h) = (3000, 40);
        write_png(&path, w, h, vec![20000; w * h * 3], None);
        let read = read_export(&path).unwrap();
        assert_eq!(read.w, EXPORT_EDGE as usize);
        assert_eq!(read.h, 27);

        // An 8-bit export is shrunk in 16 bits: averaged values between 8-bit steps survive.
        let path = dir.path().join("big8.png");
        image::RgbImage::from_fn(3000, 20, |x, _| {
            image::Rgb([(x / 3 % 2) as u8 * 2 + 100; 3])
        })
        .save(&path)
        .unwrap();
        let read = read_export(&path).unwrap();
        assert!(
            read.data
                .iter()
                .any(|v| ((v * 255.0) - (v * 255.0).round()).abs() > 0.1),
            "every value on an 8-bit step"
        );
    }
}
