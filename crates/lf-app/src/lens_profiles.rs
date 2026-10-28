//! Automatic lens profiles from the lensfun database (CC-BY-SA 3.0, downloaded once,
//! [`lf_ai::assets::LENSFUN`]). The photo's lens (EXIF) is matched to a lensfun lens by name,
//! and its calibration at the photo's focal length and aperture is turned into this app's
//! own lens sliders ([`lf_settings::lens::Lens`]), so a profile is an ordinary, editable
//! correction that renders the same everywhere the sliders do.
//!
//! lensfun's conventions: distortion and colour fringes in coordinates where half the
//! shorter side is 1, mapping the ideal radius to where the lens puts it (`ptlens`:
//! `r(a r³ + b r² + c r + 1 − a − b − c)`, `poly3`: `r(1 − k1 + k1 r²)`, `poly5`:
//! `r(1 + k1 r² + k2 r⁴)`; fringes `poly3`: `r(b r² + c r + v)` per channel); vignetting
//! `pa`: brightness × `(1 + k1 r² + k2 r⁴ + k3 r⁶)` with the corner at 1. Each is fitted by
//! least squares to the sliders' simpler shapes over the frame.

use lf_settings::lens::{CA, DISTORTION, Lens};
use std::path::Path;

pub const NO_LENS: &str =
    "This photo's EXIF does not name its lens and focal length, so no profile can be found.";
pub const LOOKING: &str = "Looking for this lens's profile…";

/// What the job needs from the open photo.
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub photo_id: i64,
    pub lens: String,
    pub maker: Option<String>,
    pub focal: f64,
    pub aperture: Option<f64>,
    /// Width per unit of height.
    pub aspect: f64,
}

/// The database, read once per run of the app.
static DATABASE: std::sync::OnceLock<Vec<Profile>> = std::sync::OnceLock::new();

/// Fetches the database into `dir` if needed (a few MB, checked), finds the photo's lens and
/// returns its name and the sliders for it.
pub fn run(dir: &Path, q: &Query) -> Result<(String, Lens), String> {
    let folder = lf_ai::assets::ensure(
        dir,
        &lf_ai::assets::LENSFUN,
        &mut |_, _| {},
        &std::sync::atomic::AtomicBool::new(false),
    )
    .map_err(|e| e.to_string())?;
    let all = DATABASE.get_or_init(|| load(&folder));
    let profile = find(all, &q.lens, q.maker.as_deref()).ok_or_else(|| {
        format!(
            "No profile was found for {} in the lensfun database.",
            q.lens
        )
    })?;
    Ok((
        profile.model.clone(),
        sliders(profile, q.aspect, q.focal, q.aperture),
    ))
}

/// One lens's calibration.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Profile {
    pub maker: String,
    pub model: String,
    /// (focal, model, coefficients) — `ptlens` a b c, `poly3` k1, `poly5` k1 k2.
    pub distortion: Vec<(f64, String, Vec<f64>)>,
    /// (focal, vr, cr, br, vb, cb, bb).
    pub tca: Vec<(f64, [f64; 6])>,
    /// (focal, aperture, k1 k2 k3).
    pub vignetting: Vec<(f64, f64, [f64; 3])>,
}

fn attr(n: roxmltree::Node, name: &str) -> f64 {
    n.attribute(name)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0)
}

/// Every lens in one lensfun XML file.
pub fn parse(xml: &str) -> Vec<Profile> {
    // The files start with a DOCTYPE naming lensfun's DTD (nothing is fetched for it).
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..roxmltree::ParsingOptions::default()
    };
    let Ok(doc) = roxmltree::Document::parse_with_options(xml, options) else {
        return Vec::new();
    };
    let text = |n: roxmltree::Node, tag: &str| {
        n.children()
            .find(|c| c.has_tag_name(tag) && c.attribute("lang").is_none())
            .and_then(|c| c.text())
            .unwrap_or_default()
            .trim()
            .to_owned()
    };
    let mut out = Vec::new();
    for lens in doc.descendants().filter(|n| n.has_tag_name("lens")) {
        let mut p = Profile {
            maker: text(lens, "maker"),
            model: text(lens, "model"),
            ..Profile::default()
        };
        let Some(cal) = lens.children().find(|c| c.has_tag_name("calibration")) else {
            continue;
        };
        for c in cal.children().filter(roxmltree::Node::is_element) {
            let focal = attr(c, "focal");
            match c.tag_name().name() {
                "distortion" => {
                    let model = c.attribute("model").unwrap_or_default().to_owned();
                    let k = match model.as_str() {
                        "ptlens" => vec![attr(c, "a"), attr(c, "b"), attr(c, "c")],
                        "poly3" => vec![attr(c, "k1")],
                        "poly5" => vec![attr(c, "k1"), attr(c, "k2")],
                        _ => continue,
                    };
                    p.distortion.push((focal, model, k));
                }
                "tca" if c.attribute("model") == Some("poly3") => {
                    let one = |name: &str, default: f64| {
                        c.attribute(name)
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(default)
                    };
                    p.tca.push((
                        focal,
                        [
                            one("vr", 1.0),
                            one("cr", 0.0),
                            one("br", 0.0),
                            one("vb", 1.0),
                            one("cb", 0.0),
                            one("bb", 0.0),
                        ],
                    ));
                }
                "vignetting" if c.attribute("model") == Some("pa") => {
                    p.vignetting.push((
                        focal,
                        attr(c, "aperture"),
                        [attr(c, "k1"), attr(c, "k2"), attr(c, "k3")],
                    ));
                }
                _ => {}
            }
        }
        if !p.model.is_empty() {
            out.push(p);
        }
    }
    out
}

/// Every lens in the database folder `dir`.
pub fn load(dir: &Path) -> Vec<Profile> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            if e.path().extension().is_some_and(|x| x == "xml")
                && let Ok(xml) = std::fs::read_to_string(e.path())
            {
                out.extend(parse(&xml));
            }
        }
    }
    out
}

/// A lens name's words: lower case, split at anything not a letter, digit or point, and
/// between letters and digits ("EF24-70mm" → "ef", "24", "70", "mm").
fn words(name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut last_digit = None;
    for ch in name.to_lowercase().chars() {
        if ch.is_alphanumeric() || ch == '.' {
            let digit = ch.is_ascii_digit() || ch == '.';
            if last_digit.is_some_and(|d| d != digit) && !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            cur.push(ch);
            last_digit = Some(digit);
        } else {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            last_digit = None;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out.retain(|w| w != "mm" && w != "f");
    out
}

/// The lensfun lens that names the EXIF `lens` (made by `maker`, when known), if one fits
/// well: every number in the lens's name must match (focal lengths, aperture), and most of
/// its words.
pub fn find<'a>(profiles: &'a [Profile], lens: &str, maker: Option<&str>) -> Option<&'a Profile> {
    let want = words(lens);
    let nums = |w: &[String]| -> Vec<String> {
        w.iter()
            .filter(|x| x.chars().next().is_some_and(|c| c.is_ascii_digit()))
            .cloned()
            .collect()
    };
    let want_nums = nums(&want);
    if want_nums.is_empty() {
        return None;
    }
    let maker = maker.map(str::to_lowercase);
    profiles
        .iter()
        .filter_map(|p| {
            let have = words(&p.model);
            if nums(&have) != want_nums {
                return None;
            }
            let shared = have.iter().filter(|w| want.contains(w)).count() as f64;
            // Words missing on either side count against it (an F-mount lens is not the Z one).
            let mut score = shared / have.len().max(want.len()).max(1) as f64;
            if score < 0.6 {
                return None;
            }
            if maker.as_deref().is_some_and(|m| {
                m.contains(&p.maker.to_lowercase()) || p.maker.to_lowercase().contains(m)
            }) {
                score += 0.1;
            }
            Some((score, p))
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, p)| p)
}

/// The entries at the focal length nearest `focal` (or the two either side, blended).
fn at_focal<T: Clone>(
    entries: &[(f64, T)],
    focal: f64,
    mix: impl Fn(&T, &T, f64) -> T,
) -> Option<T> {
    let below = entries
        .iter()
        .filter(|e| e.0 <= focal)
        .max_by(|a, b| a.0.total_cmp(&b.0));
    let above = entries
        .iter()
        .filter(|e| e.0 >= focal)
        .min_by(|a, b| a.0.total_cmp(&b.0));
    match (below, above) {
        (Some(a), Some(b)) if (b.0 - a.0).abs() > 1e-9 => {
            Some(mix(&a.1, &b.1, (focal - a.0) / (b.0 - a.0)))
        }
        (Some(a), _) => Some(a.1.clone()),
        (None, Some(b)) => Some(b.1.clone()),
        (None, None) => None,
    }
}

/// The ratio the lens puts a radius at, for a distortion entry (lensfun units).
fn distortion_ratio(model: &str, k: &[f64], r: f64) -> f64 {
    match (model, k) {
        ("ptlens", [a, b, c]) => a * r.powi(3) + b * r * r + c * r + 1.0 - a - b - c,
        ("poly3", [k1]) => 1.0 - k1 + k1 * r * r,
        ("poly5", [k1, k2]) => 1.0 + k1 * r * r + k2 * r.powi(4),
        _ => 1.0,
    }
}

/// The lens sliders `profile` calls for, on a photo `aspect` wide per unit of height, at
/// `focal` mm and `aperture` f-number.
pub fn sliders(profile: &Profile, aspect: f64, focal: f64, aperture: Option<f64>) -> Lens {
    let mut lens = Lens::default();
    // Half the shorter side is 1 in lensfun's units; the corner is `s` there.
    let long = aspect.max(1.0 / aspect.max(1e-9));
    let s = (1.0 + long * long).sqrt();
    let samples: Vec<f64> = (1..=40).map(|i| f64::from(i) / 40.0).collect();
    // Distortion: our ratio is 1 + a t (t = r² with the corner at 1).
    let ratios: Vec<(f64, (String, Vec<f64>))> = profile
        .distortion
        .iter()
        .map(|(f, m, k)| (*f, (m.clone(), k.clone())))
        .collect();
    if let Some((model, k)) = at_focal(&ratios, focal, |a, b, w| {
        if a.0 == b.0 && a.1.len() == b.1.len() {
            (
                a.0.clone(),
                a.1.iter().zip(&b.1).map(|(x, y)| x + (y - x) * w).collect(),
            )
        } else if w < 0.5 {
            a.clone()
        } else {
            b.clone()
        }
    }) {
        let (mut num, mut den) = (0.0, 0.0);
        for &t in &samples {
            let r = t.sqrt() * s;
            let target = distortion_ratio(&model, &k, r) - 1.0;
            num += t * target;
            den += t * t;
        }
        let a = num / den;
        lens.distortion = (-a / DISTORTION * 100.0).clamp(-100.0, 100.0);
    }
    // Colour fringes: each channel's mean scale over the frame.
    if let Some(t) = at_focal(&profile.tca, focal, |a, b, w| {
        let mut out = *a;
        for (o, (x, y)) in out.iter_mut().zip(a.iter().zip(b)) {
            *o = x + (y - x) * w;
        }
        out
    }) {
        let mean = |v: f64, c: f64, b: f64| {
            samples
                .iter()
                .map(|&q| {
                    let r = q * s;
                    b * r * r + c * r + v
                })
                .sum::<f64>()
                / samples.len() as f64
        };
        let red = mean(t[0], t[1], t[2]) - 1.0;
        let blue = mean(t[3], t[4], t[5]) - 1.0;
        lens.ca_red = (red / CA * 100.0).clamp(-100.0, 100.0);
        lens.ca_blue = (blue / CA * 100.0).clamp(-100.0, 100.0);
    }
    // Vignetting: our gain is 1 + v t (t = r², corner 1), against 1 / the lens's falloff.
    let at_aperture: Vec<(f64, [f64; 3])> = {
        let nearest_focal = profile
            .vignetting
            .iter()
            .map(|v| v.0)
            .min_by(|a, b| (a - focal).abs().total_cmp(&(b - focal).abs()));
        let wanted = aperture.unwrap_or(5.6);
        profile
            .vignetting
            .iter()
            .filter(|v| Some(v.0) == nearest_focal)
            .min_by(|a, b| (a.1 - wanted).abs().total_cmp(&(b.1 - wanted).abs()))
            .map(|v| vec![(v.0, v.2)])
            .unwrap_or_default()
    };
    if let Some((_, k)) = at_aperture.first() {
        let (mut num, mut den) = (0.0, 0.0);
        for &t in &samples {
            let falloff = 1.0 + k[0] * t + k[1] * t * t + k[2] * t.powi(3);
            if falloff > 1e-3 {
                num += t * (1.0 / falloff - 1.0);
                den += t * t;
            }
        }
        if den > 0.0 {
            lens.vignetting =
                (num / den * 100.0 / lf_settings::lens::VIGNETTING).clamp(-100.0, 100.0);
        }
    }
    let round = |v: f64| (v * 10.0).round() / 10.0;
    lens.distortion = round(lens.distortion);
    lens.vignetting = round(lens.vignetting);
    lens.ca_red = round(lens.ca_red);
    lens.ca_blue = round(lens.ca_blue);
    lens
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<lensdatabase version="2">
      <lens>
        <maker>Canon</maker>
        <model>Canon EF 24-70mm f/2.8L II USM</model>
        <calibration>
          <distortion model="ptlens" focal="24" a="0.0" b="-0.02" c="0.0"/>
          <distortion model="ptlens" focal="70" a="0.0" b="0.01" c="0.0"/>
          <tca model="poly3" focal="24" vr="1.0002" vb="0.9998"/>
          <vignetting model="pa" focal="24" aperture="2.8" distance="10" k1="-0.5" k2="0.1" k3="0"/>
          <vignetting model="pa" focal="24" aperture="8" distance="10" k1="-0.1" k2="0" k3="0"/>
        </calibration>
      </lens>
      <lens>
        <maker>Canon</maker>
        <model>Canon EF 70-200mm f/2.8L USM</model>
        <calibration><distortion model="poly3" focal="70" k1="0.01"/></calibration>
      </lens>
    </lensdatabase>"#;

    #[test]
    fn lenses_are_read_and_found_by_their_exif_name() {
        let all = parse(XML);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].distortion.len(), 2);
        assert_eq!(all[0].vignetting.len(), 2);
        let found = find(&all, "EF24-70mm f/2.8L II USM", Some("Canon")).unwrap();
        assert_eq!(found.model, "Canon EF 24-70mm f/2.8L II USM");
        assert!(find(&all, "EF 16-35mm f/4L IS USM", Some("Canon")).is_none());
        assert!(find(&all, "", None).is_none());
    }

    #[test]
    fn a_barrel_lens_straightens_and_a_dark_corner_lightens() {
        let all = parse(XML);
        // Barrel at 24 mm (b < 0: the edges pulled in): a positive Distortion slider.
        let wide = sliders(&all[0], 1.5, 24.0, Some(2.8));
        assert!(wide.distortion > 0.0, "{wide:?}");
        // Pincushion at 70 mm the other way.
        let long = sliders(&all[0], 1.5, 70.0, None);
        assert!(long.distortion < 0.0, "{long:?}");
        // Dark corners wide open: lightened; less so at f/8.
        assert!(wide.vignetting > 0.0);
        let stopped = sliders(&all[0], 1.5, 24.0, Some(8.0));
        assert!(stopped.vignetting < wide.vignetting);
        // Red pushed out, blue pulled in.
        assert!(wide.ca_red > 0.0 && wide.ca_blue < 0.0, "{wide:?}");
        // Halfway between 24 and 70 the distortion is blended.
        let mid = sliders(&all[0], 1.5, 47.0, None);
        assert!(mid.distortion.abs() < wide.distortion.abs());
        // Perspective is never touched.
        assert_eq!((wide.vertical, wide.horizontal), (0.0, 0.0));
    }

    #[test]
    fn lens_names_split_into_words_and_numbers() {
        assert_eq!(
            words("EF24-70mm f/2.8L II USM"),
            ["ef", "24", "70", "2.8", "l", "ii", "usm"]
        );
    }
}

/// Against the real database when `LENSFUN_DB` names its folder (skipped otherwise).
#[cfg(test)]
mod real_tests {
    use super::*;

    #[test]
    fn common_lenses_are_found_in_the_real_database() {
        let Some(dir) = std::env::var_os("LENSFUN_DB") else {
            return;
        };
        let all = load(std::path::Path::new(&dir));
        assert!(all.len() > 500, "{} lenses", all.len());
        for (exif, maker) in [
            ("EF24-70mm f/2.8L II USM", "Canon"),
            ("FE 24-70mm F2.8 GM", "Sony"),
        ] {
            let p = find(&all, exif, Some(maker));
            eprintln!("{exif} -> {:?}", p.map(|p| &p.model));
            let p = p.unwrap_or_else(|| panic!("{exif}"));
            let s = sliders(p, 1.5, 24.0, Some(2.8));
            eprintln!("   {s:?}");
            assert!(s.distortion.abs() <= 100.0);
        }
        // An F-mount lens is not taken for the Z-mount one of the same range.
        let f = find(&all, "AF-S NIKKOR 24-70mm f/2.8E ED VR", Some("Nikon"));
        assert!(
            f.is_none_or(|p| !p.model.contains(" Z ")),
            "{:?}",
            f.map(|p| &p.model)
        );
    }
}
