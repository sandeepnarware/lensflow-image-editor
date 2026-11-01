//! Colour looks from 3D LUTs (`.cube` files, as Resolve, Photoshop and most LUT packs write
//! them). Desktop-only: a photo stores which LUT and how strongly in `desktop.lut`
//! ([`LutChoice`], read and written as [`crate::lens`] is), never the table itself, which
//! lives in the catalogue's LUT folder and is loaded by name. A photo whose LUT is not
//! loaded (another machine, a deleted file) renders as if it had none.
//! A *preset* may carry its table inside itself ([`encode_table`]); applying it stores only the
//! table's name ([`name_embedded`]), and the app writes the file ([`embedded_name`]).
//!
//! The develop pass applies the LUT to the finished colour (display-referred sRGB, 0..1,
//! what LUT packs expect), before the effects (vignette, grain), mixed by the amount.

use serde::{Deserialize, Serialize};

use crate::EditSettings;

/// The largest table side accepted (65 is the common "high quality" size).
pub const MAX_SIZE: usize = 129;

/// Which LUT a photo uses, and how much of it (0..100).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LutChoice {
    pub name: String,
    #[serde(default = "full")]
    pub amount: f64,
}

fn full() -> f64 {
    100.0
}

impl LutChoice {
    /// Read `desktop.lut`; missing, unreadable or nameless is none.
    pub fn from_settings(s: &EditSettings) -> Option<LutChoice> {
        let mut c = s
            .desktop
            .as_ref()
            .and_then(|d| d.get("lut"))
            .and_then(|v| serde_json::from_value::<LutChoice>(v.clone()).ok())?;
        if c.name.trim().is_empty() {
            return None;
        }
        c.amount = if c.amount.is_finite() {
            c.amount.clamp(0.0, 100.0)
        } else {
            100.0
        };
        Some(c)
    }

    /// Store `choice` into `desktop.lut` (none is no key; an emptied `desktop` is dropped).
    pub fn write_into(choice: Option<&LutChoice>, s: &mut EditSettings) {
        match choice {
            Some(c) => {
                s.desktop.get_or_insert_with(Default::default).insert(
                    "lut".to_string(),
                    serde_json::to_value(c).expect("a LUT choice serialises"),
                );
            }
            None => {
                if let Some(desktop) = s.desktop.as_mut() {
                    desktop.remove("lut");
                    if desktop.is_empty() {
                        s.desktop = None;
                    }
                }
            }
        }
    }
}

/// A 3D table: `size`³ RGB entries, red changing fastest (the `.cube` order), each mapping
/// an input colour on the grid (0..1) to an output colour.
#[derive(Clone, Debug, PartialEq)]
pub struct Lut {
    pub size: usize,
    pub data: Vec<[f32; 3]>,
    pub title: Option<String>,
}

/// Why a `.cube` file cannot be used.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LutError {
    #[error("This is a 1D LUT; only 3D LUTs can be used as a look.")]
    OneDimensional,
    #[error("The file does not say its size (LUT_3D_SIZE).")]
    NoSize,
    #[error("The LUT's size {0} is not supported (2 to 129).")]
    BadSize(usize),
    #[error("Line {0} could not be read.")]
    BadLine(usize),
    #[error("The LUT expects input outside 0 to 1 (a log or HDR LUT), which a look cannot use.")]
    Domain,
    #[error("The file has {found} entries; a size-{size} LUT has {want}.")]
    WrongCount {
        size: usize,
        found: usize,
        want: usize,
    },
}

impl Lut {
    /// Reads a `.cube` file's text. A table whose input is not 0..1 (`DOMAIN_MIN`,
    /// `DOMAIN_MAX`: a log or HDR LUT) is refused, as its colours would come out wrong.
    pub fn parse_cube(text: &str) -> Result<Lut, LutError> {
        let mut size = None;
        let mut title = None;
        let mut data = Vec::new();
        let mut min = [0f32; 3];
        let mut max = [1f32; 3];
        for (i, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut words = line.split_whitespace();
            let first = words.next().unwrap_or_default();
            let three = |words: std::str::SplitWhitespace| -> Result<[f32; 3], LutError> {
                let v: Vec<f32> = words
                    .map(str::parse::<f32>)
                    .collect::<Result<_, _>>()
                    .map_err(|_| LutError::BadLine(i + 1))?;
                match v.as_slice() {
                    [r, g, b] if r.is_finite() && g.is_finite() && b.is_finite() => {
                        Ok([*r, *g, *b])
                    }
                    _ => Err(LutError::BadLine(i + 1)),
                }
            };
            match first.to_ascii_uppercase().as_str() {
                "TITLE" => {
                    title = Some(line[5..].trim().trim_matches('"').to_owned());
                }
                "LUT_1D_SIZE" => return Err(LutError::OneDimensional),
                "LUT_3D_SIZE" => {
                    let n: usize = words
                        .next()
                        .and_then(|w| w.parse().ok())
                        .ok_or(LutError::BadLine(i + 1))?;
                    if !(2..=MAX_SIZE).contains(&n) {
                        return Err(LutError::BadSize(n));
                    }
                    size = Some(n);
                    data.reserve(n * n * n);
                }
                "DOMAIN_MIN" => min = three(words)?,
                "DOMAIN_MAX" => max = three(words)?,
                // Other keywords (LUT_3D_INPUT_RANGE, …) are skipped.
                w if w.starts_with(|c: char| c.is_ascii_alphabetic()) => {}
                _ => {
                    let entry = three(line.split_whitespace())?;
                    data.push(entry);
                }
            }
        }
        let size = size.ok_or(LutError::NoSize)?;
        let want = size * size * size;
        if data.len() != want {
            return Err(LutError::WrongCount {
                size,
                found: data.len(),
                want,
            });
        }
        if min != [0.0; 3] || max != [1.0; 3] {
            return Err(LutError::Domain);
        }
        Ok(Lut { size, data, title })
    }

    /// The table as `.cube` text (red fastest, as [`Lut::parse_cube`] reads it).
    pub fn to_cube(&self) -> String {
        let mut text = String::with_capacity(self.data.len() * 24 + 64);
        if let Some(t) = &self.title {
            text.push_str(&format!(
                "TITLE \"{t}\"
"
            ));
        }
        text.push_str(&format!(
            "LUT_3D_SIZE {}
",
            self.size
        ));
        for p in &self.data {
            text.push_str(&format!(
                "{:.6} {:.6} {:.6}
",
                p[0], p[1], p[2]
            ));
        }
        text
    }

    /// The identity table of `size` (each grid point maps to itself).
    pub fn identity(size: usize) -> Lut {
        let step = |i: usize| i as f32 / (size - 1) as f32;
        let mut data = Vec::with_capacity(size * size * size);
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    data.push([step(r), step(g), step(b)]);
                }
            }
        }
        Lut {
            size,
            data,
            title: None,
        }
    }

    /// The table's colour for `rgb` (0..1), trilinearly between grid points: what the
    /// shader's lookup gives, for tests and previews.
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let n = self.size;
        let pos = rgb.map(|v| v.clamp(0.0, 1.0) * (n - 1) as f32);
        let lo = pos.map(|p| (p.floor() as usize).min(n - 2));
        let t = [
            pos[0] - lo[0] as f32,
            pos[1] - lo[1] as f32,
            pos[2] - lo[2] as f32,
        ];
        let at = |r: usize, g: usize, b: usize| self.data[(b * n + g) * n + r];
        let mut out = [0f32; 3];
        for (dr, wr) in [(0, 1.0 - t[0]), (1, t[0])] {
            for (dg, wg) in [(0, 1.0 - t[1]), (1, t[1])] {
                for (db, wb) in [(0, 1.0 - t[2]), (1, t[2])] {
                    let v = at(lo[0] + dr, lo[1] + dg, lo[2] + db);
                    let w = wr * wg * wb;
                    for c in 0..3 {
                        out[c] += v[c] * w;
                    }
                }
            }
        }
        out
    }
}

/// The name a photo stores for an embedded table: its content hash, so the same table is the
/// same file in every catalogue.
pub fn embedded_name(hash: &str) -> String {
    format!("lr-{hash}")
}

/// Fixed-point scale of an embedded table's 16-bit values.
const Q: f32 = 65535.0;

fn quantise(lut: &Lut) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(lut.data.len() * 6);
    for p in &lut.data {
        for c in p {
            let q = (c.clamp(0.0, 1.0) * Q).round() as u16;
            bytes.extend_from_slice(&q.to_le_bytes());
        }
    }
    bytes
}

fn hash_of(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A table as it travels inside a preset (`desktop.lut`): `{size, table, hash}`, the table
/// as base64 of 16-bit little-endian RGB values (a 17 size table is about 39 KB), the hash
/// the first 16 hex characters of the SHA-256 of those bytes.
pub fn encode_table(lut: &Lut) -> serde_json::Value {
    let bytes = quantise(lut);
    serde_json::json!({
        "size": lut.size,
        "hash": hash_of(&bytes),
        "table": crate::b64::base64_encode(&bytes),
    })
}

/// Reads [`encode_table`]'s value; none when anything is missing, the size is out of range,
/// the length is wrong or the hash does not match the bytes.
pub fn decode_table(v: &serde_json::Value) -> Option<Lut> {
    let size = usize::try_from(v.get("size")?.as_u64()?).ok()?;
    if !(2..=MAX_SIZE).contains(&size) {
        return None;
    }
    let bytes = crate::b64::base64_decode(v.get("table")?.as_str()?)?;
    if bytes.len() != size * size * size * 6 || hash_of(&bytes) != v.get("hash")?.as_str()? {
        return None;
    }
    let value = |i: usize| f32::from(u16::from_le_bytes([bytes[i * 2], bytes[i * 2 + 1]])) / Q;
    let data = (0..size * size * size)
        .map(|n| [value(n * 3), value(n * 3 + 1), value(n * 3 + 2)])
        .collect();
    Some(Lut {
        size,
        data,
        title: None,
    })
}

/// The table a preset carries in `desktop.lut`, if it carries a readable one.
pub fn embedded_in(preset: &serde_json::Value) -> Option<Lut> {
    decode_table(preset.get("desktop")?.get("lut")?)
}

/// `preset` with an embedded table in `desktop.lut` replaced by the usual
/// `{name, amount}` choice naming it ([`embedded_name`]), so a photo never stores the table.
/// A preset without a table, or with a corrupt one (its `lut` is dropped: there is nothing to
/// name), is returned as it is. Pure: [`crate::lut`]'s file is written by the app.
pub fn name_embedded(preset: &serde_json::Value) -> serde_json::Value {
    let mut out = preset.clone();
    let Some(lut) = out.pointer_mut("/desktop/lut") else {
        return out;
    };
    if lut.get("table").is_none() {
        return out;
    }
    match lut
        .get("hash")
        .and_then(|h| h.as_str())
        .filter(|_| decode_table(lut).is_some())
    {
        Some(hash) => {
            let amount = lut.get("amount").cloned().unwrap_or(100.0.into());
            *lut = serde_json::json!({"name": embedded_name(hash), "amount": amount});
        }
        None => {
            if let Some(d) = out.get_mut("desktop").and_then(|d| d.as_object_mut()) {
                d.remove("lut");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cube_file_is_read() {
        let text = "# made by hand\nTITLE \"Warm\"\nLUT_3D_SIZE 2\nDOMAIN_MIN 0 0 0\n\
                    0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n";
        let lut = Lut::parse_cube(text).unwrap();
        assert_eq!(lut.size, 2);
        assert_eq!(lut.title.as_deref(), Some("Warm"));
        assert_eq!(
            lut,
            Lut {
                title: Some("Warm".into()),
                ..Lut::identity(2)
            }
        );
        let got = lut.apply([0.25, 0.5, 0.75]);
        for (g, w) in got.iter().zip([0.25, 0.5, 0.75]) {
            assert!((g - w).abs() < 1e-6);
        }
    }

    #[test]
    fn bad_files_say_why() {
        assert_eq!(
            Lut::parse_cube("LUT_1D_SIZE 4\n0 0 0\n"),
            Err(LutError::OneDimensional)
        );
        assert_eq!(Lut::parse_cube("0 0 0\n"), Err(LutError::NoSize));
        assert_eq!(
            Lut::parse_cube("LUT_3D_SIZE 500\n"),
            Err(LutError::BadSize(500))
        );
        assert_eq!(
            Lut::parse_cube("LUT_3D_SIZE 2\n0 0 0\n1 x 0\n"),
            Err(LutError::BadLine(3))
        );
        assert!(matches!(
            Lut::parse_cube("LUT_3D_SIZE 2\n0 0 0\n"),
            Err(LutError::WrongCount {
                found: 1,
                want: 8,
                ..
            })
        ));
    }

    #[test]
    fn the_choice_round_trips_and_is_made_safe() {
        let mut s = EditSettings::default_settings();
        assert_eq!(LutChoice::from_settings(&s), None);
        let c = LutChoice {
            name: "Teal & orange".into(),
            amount: 140.0,
        };
        LutChoice::write_into(Some(&c), &mut s);
        assert_eq!(LutChoice::from_settings(&s).unwrap().amount, 100.0);
        LutChoice::write_into(None, &mut s);
        assert!(s.desktop.is_none());
    }

    fn learned(size: usize) -> Lut {
        let mut lut = Lut::identity(size);
        for (i, p) in lut.data.iter_mut().enumerate() {
            p[0] = (p[0] * 0.9 + 0.03).clamp(0.0, 1.0);
            p[1] = (p[1] + (i % 7) as f32 * 0.002).clamp(0.0, 1.0);
        }
        lut
    }

    #[test]
    fn an_embedded_table_round_trips() {
        let lut = learned(17);
        let v = encode_table(&lut);
        assert!(serde_json::to_vec(&v).unwrap().len() < 40_000);
        assert_eq!(v["size"], 17);
        let hash = v["hash"].as_str().unwrap();
        assert_eq!(hash.len(), 16);
        assert_eq!(embedded_name(hash), format!("lr-{hash}"));
        let back = decode_table(&v).unwrap();
        assert_eq!(back.size, 17);
        for (a, b) in back.data.iter().zip(&lut.data) {
            for c in 0..3 {
                assert!((a[c] - b[c]).abs() <= 1.0 / 65535.0 + 1e-6);
            }
        }
        assert_eq!(encode_table(&back)["hash"], v["hash"]);
    }

    #[test]
    fn a_corrupt_table_is_none() {
        let good = encode_table(&learned(5));
        let mut short = good.clone();
        short["table"] = "AAAA".into();
        assert!(decode_table(&short).is_none());
        let mut huge = good.clone();
        huge["size"] = 100000.into();
        assert!(decode_table(&huge).is_none());
        let mut junk = good.clone();
        junk["table"] = "!!!".into();
        assert!(decode_table(&junk).is_none());
        let mut forged = good.clone();
        forged["hash"] = "0000000000000000".into();
        assert!(decode_table(&forged).is_none());
        assert!(decode_table(&serde_json::json!({"name": "x"})).is_none());
    }

    #[test]
    fn a_cube_written_reads_back() {
        let lut = learned(5);
        let back = Lut::parse_cube(&lut.to_cube()).unwrap();
        assert_eq!(back.size, 5);
        for (a, b) in back.data.iter().zip(&lut.data) {
            for c in 0..3 {
                assert!((a[c] - b[c]).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn naming_a_preset_table_keeps_only_name_and_amount() {
        let mut preset =
            serde_json::json!({"v": 2, "desktop": {"lut": encode_table(&learned(5)), "other": 1}});
        preset["desktop"]["lut"]["amount"] = 60.into();
        let hash = preset["desktop"]["lut"]["hash"]
            .as_str()
            .unwrap()
            .to_owned();
        let named = name_embedded(&preset);
        assert_eq!(
            named["desktop"]["lut"],
            serde_json::json!({"name": format!("lr-{hash}"), "amount": 60})
        );
        assert_eq!(named["desktop"]["other"], 1);
        assert!(embedded_in(&preset).is_some());
        assert!(embedded_in(&named).is_none());
        let plain = serde_json::json!({"desktop": {"lut": {"name": "Film", "amount": 50}}});
        assert_eq!(name_embedded(&plain), plain);
        let mut bad = preset.clone();
        bad["desktop"]["lut"]["table"] = "AAAA".into();
        assert!(name_embedded(&bad)["desktop"].get("lut").is_none());
    }
}
