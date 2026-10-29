//! AI keywording: which of a fixed list of words (`data/keywords.txt`: beach, cake, bride...)
//! a photo shows, for the library's search. CLIP ViT-B/32 (OpenAI, MIT) describes the photo
//! as 512 numbers in the same space as text; each word's text description was worked out once
//! ahead of time (`scripts/keywords/embed.py`, averaged over a few prompts) and ships with
//! the app (`data/keywords.f16`), so only CLIP's image model is downloaded and no tokenizer is
//! needed.
//!
//! A word is suggested when the photo is like it ([`MIN_COSINE`]) and it stands out from the
//! other words (its share of CLIP's softmax over the list, [`MIN_SHARE`]); at most
//! [`MAX_WORDS`] per photo, best first.

use crate::assets::{self, Asset};
use crate::{AiError, Engine, Rgb};
use std::sync::OnceLock;

pub const KEYWORD_MODELS: [Asset; 1] = [assets::CLIP_VISION];

/// The long edge photos are rendered at for keywording (CLIP sees 224 × 224).
pub const SCAN_EDGE: u32 = 512;
/// How alike (cosine) a photo and a word must be.
pub const MIN_COSINE: f32 = 0.27;
/// The least share of the softmax over every word a suggestion needs.
pub const MIN_SHARE: f32 = 0.08;
pub const MAX_WORDS: usize = 5;
/// CLIP's learned softmax temperature.
const LOGIT_SCALE: f32 = 100.0;
const EDGE: usize = 224;
const LENGTH: usize = 512;
const MEAN: [f32; 3] = [0.481_454_66, 0.457_827_5, 0.408_210_73];
const STD: [f32; 3] = [0.268_629_54, 0.261_302_6, 0.275_777_1];

const WORDS: &str = include_str!("../data/keywords.txt");
const TABLE: &[u8] = include_bytes!("../data/keywords.f16");

/// The keywords, in the table's order.
pub fn words() -> &'static [&'static str] {
    static WORDS_LIST: OnceLock<Vec<&'static str>> = OnceLock::new();
    WORDS_LIST.get_or_init(|| {
        WORDS
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .filter_map(|l| l.split('\t').next())
            .map(str::trim)
            .collect()
    })
}

/// An IEEE half as an `f32`.
fn half(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from((bits >> 10) & 0x1f);
    let frac = f32::from(bits & 0x3ff);
    sign * match exp {
        0 => frac * 2f32.powi(-24),
        31 if frac == 0.0 => f32::INFINITY,
        31 => f32::NAN,
        e => (1.0 + frac / 1024.0) * 2f32.powi(e - 15),
    }
}

/// Each word's text description, [`LENGTH`] numbers, unit length.
fn table() -> &'static [Vec<f32>] {
    static ROWS: OnceLock<Vec<Vec<f32>>> = OnceLock::new();
    ROWS.get_or_init(|| {
        TABLE
            .as_chunks::<2>()
            .0
            .chunks(LENGTH)
            .map(|row| row.iter().map(|b| half(u16::from_le_bytes(*b))).collect())
            .collect()
    })
}

/// A word suggested for a photo, and how sure (its softmax share, 0..1).
#[derive(Debug, Clone, PartialEq)]
pub struct Suggested {
    pub word: &'static str,
    pub score: f32,
}

/// The words a photo described as `image` (unit length) is suggested.
pub fn suggest(image: &[f32]) -> Vec<Suggested> {
    let cosines: Vec<f32> = table()
        .iter()
        .map(|t| t.iter().zip(image).map(|(a, b)| a * b).sum())
        .collect();
    pick(&cosines)
}

/// The suggestions from each word's cosine with the photo.
fn pick(cosines: &[f32]) -> Vec<Suggested> {
    let top = cosines.iter().copied().fold(f32::MIN, f32::max);
    let exps: Vec<f32> = cosines
        .iter()
        .map(|c| ((c - top) * LOGIT_SCALE).exp())
        .collect();
    let sum: f32 = exps.iter().sum();
    let mut out: Vec<Suggested> = cosines
        .iter()
        .zip(&exps)
        .zip(words())
        .filter(|((c, e), _)| **c >= MIN_COSINE && **e / sum >= MIN_SHARE)
        .map(|((_, e), w)| Suggested {
            word: w,
            score: e / sum,
        })
        .collect();
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    out.truncate(MAX_WORDS);
    out
}

/// `img` as CLIP takes it: the shorter side 224, the middle 224 × 224, normalised, planar.
pub fn input(img: &Rgb) -> Vec<f32> {
    let scale = EDGE as f32 / img.w.min(img.h).max(1) as f32;
    let (w, h) = (
        ((img.w as f32 * scale).round() as usize).max(EDGE),
        ((img.h as f32 * scale).round() as usize).max(EDGE),
    );
    let middle = img
        .resized(w, h)
        .crop((w - EDGE) / 2, (h - EDGE) / 2, EDGE, EDGE);
    let n = EDGE * EDGE;
    let mut out = middle.planar();
    for c in 0..3 {
        for v in &mut out[c * n..(c + 1) * n] {
            *v = (*v - MEAN[c]) / STD[c];
        }
    }
    out
}

/// Keywords photos with CLIP's image model loaded once.
pub struct Tagger {
    vision: ort::session::Session,
}

impl Tagger {
    /// The model must be ready ([`Engine::prepare`] with [`KEYWORD_MODELS`]).
    pub fn new(engine: &Engine) -> Result<Tagger, AiError> {
        Ok(Tagger {
            vision: engine.session(&assets::CLIP_VISION)?,
        })
    }

    /// The words suggested for `img` (as the photo looks, about [`SCAN_EDGE`]).
    pub fn tag(&mut self, img: &Rgb) -> Result<Vec<Suggested>, AiError> {
        let (_, mut e) = crate::run_one(
            &mut self.vision,
            "pixel_values",
            [1, 3, EDGE, EDGE],
            input(img),
        )?;
        if e.len() != LENGTH {
            return Err(AiError::Model(format!(
                "the keyword model answered {} numbers, not {LENGTH}",
                e.len()
            )));
        }
        let n = e.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-12);
        e.iter_mut().for_each(|v| *v /= n);
        Ok(suggest(&e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_word_has_a_unit_description() {
        assert_eq!(words().len(), table().len());
        assert!(words().len() > 150);
        assert!(words().contains(&"beach") && words().contains(&"cake"));
        for row in table() {
            assert_eq!(row.len(), LENGTH);
            let n = row.iter().map(|v| v * v).sum::<f32>().sqrt();
            assert!((n - 1.0).abs() < 1e-2, "{n}");
        }
    }

    #[test]
    fn halves_decode() {
        assert_eq!(half(0x3c00), 1.0);
        assert_eq!(half(0xc000), -2.0);
        assert_eq!(half(0x3555), 0.333_251_95);
        assert_eq!(half(0x0001), 2f32.powi(-24));
    }

    #[test]
    fn a_word_is_suggested_when_alike_and_standing_out() {
        let beach = words().iter().position(|w| *w == "beach").unwrap();
        let sea = words().iter().position(|w| *w == "sea").unwrap();
        let mut cos = vec![0.15; words().len()];
        cos[beach] = 0.31;
        cos[sea] = 0.30;
        let got: Vec<&str> = pick(&cos).iter().map(|s| s.word).collect();
        assert_eq!(got, vec!["beach", "sea"]);
        // Nothing alike enough: nothing suggested, however it stands out.
        let mut cos = vec![0.10; words().len()];
        cos[beach] = 0.25;
        assert!(pick(&cos).is_empty());
        // The photo of a beach matches the word beach best.
        assert_eq!(suggest(&table()[beach])[0].word, "beach");
    }

    #[test]
    fn the_input_is_the_middle_square() {
        let mut img = Rgb::new(448, 224);
        // The left quarter red: cut off by the crop.
        for y in 0..224 {
            for x in 0..112 {
                img.data[(y * 448 + x) * 3] = 1.0;
            }
        }
        let v = input(&img);
        assert_eq!(v.len(), 3 * EDGE * EDGE);
        let red_at_left_edge = v[EDGE * 100];
        assert!((red_at_left_edge - (0.0 - MEAN[0]) / STD[0]).abs() < 1e-4);
    }
}
