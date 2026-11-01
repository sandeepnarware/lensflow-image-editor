//! Reading Lightroom preset files into a neutral [`LrPreset`].
//!
//! Lightroom keeps a preset as XMP (`.xmp`, Camera Raw settings in the `crs:` namespace) or,
//! in older versions, as a Lua-like `.lrtemplate`. Both are reduced here to the same thing: a
//! name, a kind and a JSON map keyed by the `crs:` local names (`"Exposure2012"`). Nothing
//! here knows our settings; mapping the keys is a later step, so unknown keys survive reading.
//!
//! Files come from strangers, so reading is bounded: a size cap, no DTDs (roxmltree refuses
//! them) and a depth cap on the `.lrtemplate` reader.

use serde_json::{Map, Number, Value};

/// Files larger than this are refused; real presets are a few KB.
const MAX_BYTES: usize = 1 << 20;
/// Deepest table nesting the `.lrtemplate` reader follows.
const MAX_DEPTH: usize = 64;

/// The Camera Raw settings namespace.
const CRS_NS: &str = "http://ns.adobe.com/camera-raw-settings/1.0/";
const RDF_NS: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";

/// Whether a file is a develop preset or a profile (a "Look").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LrKind {
    Preset,
    Profile,
}

/// A Lightroom preset read from disk, before any mapping to our settings.
#[derive(Debug, Clone, PartialEq)]
pub struct LrPreset {
    /// The preset's own name; empty when the file has none (the caller uses the file name).
    pub name: String,
    /// The group the file names (XMP only; a `.lrtemplate`'s group is its folder).
    pub group: Option<String>,
    pub kind: LrKind,
    /// `crs:ProcessVersion`, such as `"11.0"`.
    pub process_version: Option<String>,
    /// The settings, keyed by `crs:` local name. `Name`, `Group`, `PresetType` and
    /// `ProcessVersion` are taken out into the fields above.
    pub settings: Map<String, Value>,
}

/// Why a preset file could not be read. The messages are ready to show.
#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("That file is not readable XML, so it is not a Lightroom preset.")]
    NotXml,
    #[error("That file does not hold Lightroom develop settings.")]
    NotAPreset,
    #[error("That file is too large to be a Lightroom preset.")]
    TooLarge,
}

/// Read `bytes` as a preset by the file name's extension (`.xmp` or `.lrtemplate`).
pub fn read_any(file_name: &str, bytes: &[u8]) -> Result<LrPreset, ReadError> {
    let lower = file_name.to_ascii_lowercase();
    if lower.ends_with(".xmp") {
        read_xmp(bytes)
    } else if lower.ends_with(".lrtemplate") {
        if bytes.len() > MAX_BYTES {
            return Err(ReadError::TooLarge);
        }
        let text = decode(bytes).ok_or(ReadError::NotAPreset)?;
        read_lrtemplate(&text)
    } else {
        Err(ReadError::NotAPreset)
    }
}

/// Decode UTF-8 (with or without a BOM) or BOM-marked UTF-16 text.
fn decode(bytes: &[u8]) -> Option<String> {
    let utf16 = |rest: &[u8], le: bool| {
        if !rest.len().is_multiple_of(2) {
            return None;
        }
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| {
                if le {
                    u16::from_le_bytes([c[0], c[1]])
                } else {
                    u16::from_be_bytes([c[0], c[1]])
                }
            })
            .collect();
        String::from_utf16(&units).ok()
    };
    match bytes {
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8(rest.to_vec()).ok(),
        [0xFF, 0xFE, rest @ ..] => utf16(rest, true),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, false),
        _ => String::from_utf8(bytes.to_vec()).ok(),
    }
}

// ---------------------------------------------------------------- XMP

/// Read a `.xmp` preset.
pub fn read_xmp(bytes: &[u8]) -> Result<LrPreset, ReadError> {
    if bytes.len() > MAX_BYTES {
        return Err(ReadError::TooLarge);
    }
    let text = decode(bytes).ok_or(ReadError::NotXml)?;
    let doc = roxmltree::Document::parse(&text).map_err(|_| ReadError::NotXml)?;
    let description = doc
        .descendants()
        .find(|n| is_rdf(n, "Description") && carries_crs(n))
        .ok_or(ReadError::NotAPreset)?;
    let Value::Object(mut settings) = settings_object(description) else {
        return Err(ReadError::NotAPreset);
    };
    Ok(finish(&mut settings))
}

fn is_rdf(n: &roxmltree::Node, local: &str) -> bool {
    n.is_element() && n.tag_name().namespace() == Some(RDF_NS) && n.tag_name().name() == local
}

/// Whether the element has any `crs:` attribute or child.
fn carries_crs(n: &roxmltree::Node) -> bool {
    n.attributes().any(|a| a.namespace() == Some(CRS_NS))
        || n.children()
            .any(|c| c.is_element() && c.tag_name().namespace() == Some(CRS_NS))
}

/// Attributes that are RDF/XML plumbing rather than settings.
fn is_plumbing(a: &roxmltree::Attribute) -> bool {
    matches!(a.namespace(), Some(RDF_NS) | Some(XML_NS))
}

/// An `rdf:Description` (or a `parseType="Resource"` element) as an object: its attributes
/// and its child elements, keyed by local name.
fn description_object(n: roxmltree::Node) -> Value {
    let mut map = Map::new();
    for a in n.attributes().filter(|a| !is_plumbing(a)) {
        map.insert(a.name().to_owned(), scalar(a.value()));
    }
    for c in n.children().filter(|c| c.is_element()) {
        map.insert(c.tag_name().name().to_owned(), element_value(c, false));
    }
    Value::Object(map)
}

/// The value of a property element. `in_list` keeps plain text as a string, which is how
/// `rdf:Seq` items such as curve points stay `"0, 0"` rather than becoming numbers.
fn element_value(n: roxmltree::Node, in_list: bool) -> Value {
    let kids: Vec<_> = n.children().filter(|c| c.is_element()).collect();
    if let Some(alt) = kids.iter().find(|c| is_rdf(c, "Alt")) {
        let items: Vec<_> = alt.children().filter(|c| is_rdf(c, "li")).collect();
        let pick = items
            .iter()
            .find(|li| li.attribute((XML_NS, "lang")) == Some("x-default"))
            .or(items.first());
        return Value::String(pick.map(text_of).unwrap_or_default());
    }
    if let Some(list) = kids.iter().find(|c| is_rdf(c, "Seq") || is_rdf(c, "Bag")) {
        return Value::Array(
            list.children()
                .filter(|c| is_rdf(c, "li"))
                .map(|li| element_value(li, true))
                .collect(),
        );
    }
    if let Some(d) = kids.iter().find(|c| is_rdf(c, "Description")) {
        return description_object(*d);
    }
    if n.attributes().any(|a| !is_plumbing(&a)) || !kids.is_empty() {
        return description_object(n);
    }
    let text = text_of(&n);
    if in_list {
        Value::String(text)
    } else {
        scalar(&text)
    }
}

fn text_of(n: &roxmltree::Node) -> String {
    n.text().unwrap_or("").trim().to_owned()
}

/// XMP writes everything as text: `"True"`, `"+0.50"`, `"+20"`.
fn scalar(s: &str) -> Value {
    match s {
        "True" | "true" => return Value::Bool(true),
        "False" | "false" => return Value::Bool(false),
        _ => {}
    }
    if let Ok(i) = s.parse::<i64>() {
        return Value::from(i);
    }
    if let Ok(f) = s.parse::<f64>()
        && let Some(n) = Number::from_f64(f)
    {
        return Value::Number(n);
    }
    Value::String(s.to_owned())
}

/// Move the identity keys out of `settings` into an [`LrPreset`].
fn finish(settings: &mut Map<String, Value>) -> LrPreset {
    let take_text = |settings: &mut Map<String, Value>, key: &str| match settings.remove(key) {
        Some(Value::String(s)) if !s.is_empty() => Some(s),
        _ => None,
    };
    let name = take_text(settings, "Name").unwrap_or_default();
    let group = take_text(settings, "Group");
    let preset_type = take_text(settings, "PresetType");
    let process_version = match settings.remove("ProcessVersion") {
        Some(Value::String(s)) => Some(s),
        // A bare number lost its spelling ("11.0" read as 11); give back one decimal.
        Some(Value::Number(n)) => Some(match n.as_f64() {
            Some(f) if n.is_f64() => format!("{f:?}"),
            _ => format!("{n}.0"),
        }),
        _ => None,
    };
    let kind = if preset_type.as_deref() == Some("Look") || settings.contains_key("ProfileName") {
        LrKind::Profile
    } else {
        LrKind::Preset
    };
    LrPreset {
        name,
        group,
        kind,
        process_version,
        settings: std::mem::take(settings),
    }
}

// ---------------------------------------------------------------- .lrtemplate

/// Read a `.lrtemplate` preset (`s = { ... value = { settings = { ... } } }`).
pub fn read_lrtemplate(text: &str) -> Result<LrPreset, ReadError> {
    if text.len() > MAX_BYTES {
        return Err(ReadError::TooLarge);
    }
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut p = Lua {
        s: text.as_bytes(),
        i: 0,
    };
    let root = p.top().ok_or(ReadError::NotAPreset)?;
    let s = root.get("s").ok_or(ReadError::NotAPreset)?;
    let mut settings = match s.get("value").and_then(|v| v.get("settings")) {
        Some(Value::Object(m)) => m.clone(),
        _ => return Err(ReadError::NotAPreset),
    };
    let title = |key: &str| match s.get(key) {
        Some(Value::String(t)) if !t.is_empty() => Some(t.clone()),
        _ => None,
    };
    let name = title("title")
        .or_else(|| title("internalName"))
        .unwrap_or_default();
    let mut preset = finish(&mut settings);
    preset.name = name;
    Ok(preset)
}

/// A small recursive-descent reader for the Lua subset `.lrtemplate` files use.
struct Lua<'a> {
    s: &'a [u8],
    i: usize,
}

impl Lua<'_> {
    /// `name = value` pairs until the end of the text.
    fn top(&mut self) -> Option<Value> {
        let mut map = Map::new();
        loop {
            self.ws();
            if self.i >= self.s.len() {
                return Some(Value::Object(map));
            }
            let key = self.ident()?;
            self.ws();
            self.eat(b'=')?;
            map.insert(key, self.value(0)?);
            self.ws();
            if self.peek() == Some(b';') {
                self.i += 1;
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn eat(&mut self, c: u8) -> Option<()> {
        (self.peek() == Some(c)).then(|| self.i += 1)
    }

    /// Skip whitespace and `--` line comments.
    fn ws(&mut self) {
        loop {
            while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
                self.i += 1;
            }
            if self.s[self.i..].starts_with(b"--") {
                while self.peek().is_some_and(|c| c != b'\n') {
                    self.i += 1;
                }
            } else {
                return;
            }
        }
    }

    fn ident(&mut self) -> Option<String> {
        let start = self.i;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            self.i += 1;
        }
        (self.i > start).then(|| String::from_utf8_lossy(&self.s[start..self.i]).into_owned())
    }

    fn string(&mut self) -> Option<String> {
        let quote = self.peek().filter(|c| matches!(c, b'"' | b'\''))?;
        self.i += 1;
        let mut out = Vec::new();
        loop {
            let c = self.peek()?;
            self.i += 1;
            match c {
                c if c == quote => return Some(String::from_utf8_lossy(&out).into_owned()),
                b'\\' => {
                    let e = self.peek()?;
                    self.i += 1;
                    out.push(match e {
                        b'n' => b'\n',
                        b't' => b'\t',
                        b'r' => b'\r',
                        other => other,
                    });
                }
                c => out.push(c),
            }
        }
    }

    fn value(&mut self, depth: usize) -> Option<Value> {
        if depth > MAX_DEPTH {
            return None;
        }
        self.ws();
        match self.peek()? {
            b'{' => self.table(depth),
            b'"' | b'\'' => self.string().map(Value::String),
            c if matches!(c, b'-' | b'+' | b'.') || c.is_ascii_digit() => self.number(),
            _ => match self.ident()?.as_str() {
                "true" => Some(Value::Bool(true)),
                "false" => Some(Value::Bool(false)),
                "nil" => Some(Value::Null),
                "ZSTR" => {
                    self.ws();
                    // `$$$/AgPreset/Moody=Moody`: the text after the first `=` is the string.
                    let raw = self.string()?;
                    let text = match raw.strip_prefix("$$$/") {
                        Some(_) => raw.split_once('=').map_or(raw.as_str(), |(_, t)| t),
                        None => raw.as_str(),
                    };
                    Some(Value::String(text.to_owned()))
                }
                _ => None,
            },
        }
    }

    fn number(&mut self) -> Option<Value> {
        let start = self.i;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_digit() || matches!(c, b'-' | b'+' | b'.' | b'e' | b'E'))
        {
            self.i += 1;
        }
        let t = std::str::from_utf8(&self.s[start..self.i]).ok()?;
        Some(scalar(t)).filter(Value::is_number)
    }

    /// `{ key = value, ... }` as an object, `{ a, b, ... }` as an array.
    fn table(&mut self, depth: usize) -> Option<Value> {
        self.eat(b'{')?;
        let mut keyed = Map::new();
        let mut listed = Vec::new();
        loop {
            self.ws();
            if self.peek()? == b'}' {
                self.i += 1;
                break;
            }
            let save = self.i;
            let key = if self.peek() == Some(b'[') {
                self.i += 1;
                self.ws();
                let k = self.string()?;
                self.ws();
                self.eat(b']')?;
                Some(k)
            } else {
                self.ident()
            };
            self.ws();
            if let (Some(k), Some(b'=')) = (key, self.peek()) {
                self.i += 1;
                keyed.insert(k, self.value(depth + 1)?);
            } else {
                self.i = save;
                listed.push(self.value(depth + 1)?);
            }
            self.ws();
            if matches!(self.peek(), Some(b',') | Some(b';')) {
                self.i += 1;
            }
        }
        if listed.is_empty() {
            Some(pair_curves(keyed))
        } else {
            Some(Value::Array(listed))
        }
    }
}

/// `.lrtemplate` writes a curve as flat numbers (`{ 0, 0, 255, 255 }`); XMP writes pairs of
/// text (`"0, 0"`). Make the first look like the second so one mapping serves both.
fn pair_curves(map: Map<String, Value>) -> Value {
    let mut out = Map::new();
    for (k, v) in map {
        let v = match &v {
            Value::Array(a)
                if k.starts_with("ToneCurve")
                    && !a.is_empty()
                    && a.len().is_multiple_of(2)
                    && a.iter().all(Value::is_number) =>
            {
                Value::Array(
                    a.chunks(2)
                        .map(|p| Value::String(format!("{}, {}", num(&p[0]), num(&p[1]))))
                        .collect(),
                )
            }
            _ => v,
        };
        out.insert(k, v);
    }
    Value::Object(out)
}

fn num(v: &Value) -> String {
    match v.as_f64() {
        Some(f) if f.fract() == 0.0 => format!("{}", f as i64),
        Some(f) => format!("{f}"),
        None => String::new(),
    }
}

/// The top-level `rdf:Description` as settings: only `crs:` attributes and children, so
/// `xmp:`, `dc:` and `photoshop:` extras stay out. `Name`, `Group` and `PresetType` are
/// kept as text, so a preset called "1985" is not read as a number.
fn settings_object(n: roxmltree::Node) -> Value {
    let mut map = Map::new();
    for a in n.attributes().filter(|a| a.namespace() == Some(CRS_NS)) {
        let v = match a.name() {
            "Name" | "Group" | "PresetType" => Value::String(a.value().to_owned()),
            _ => scalar(a.value()),
        };
        map.insert(a.name().to_owned(), v);
    }
    for c in n
        .children()
        .filter(|c| c.is_element() && c.tag_name().namespace() == Some(CRS_NS))
    {
        map.insert(c.tag_name().name().to_owned(), element_value(c, false));
    }
    Value::Object(map)
}
