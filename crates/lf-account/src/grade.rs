use crate::{AccountError, Client};
use lf_settings::looks::Look;
use lf_settings::{EditSettings, HslBand};
use reqwest::blocking::multipart::{Form, Part};
use serde_json::{Map, Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Extract,
    Review,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Mode::Extract => "extract",
            Mode::Review => "review",
        }
    }
}

const MAX_BYTES: usize = 700 * 1024;
const MAX_EXAMPLES: usize = 16;
/// The server's limits on the JSON text fields (`GradeRequest`: `max:16384`, `max:8192`,
/// counted in characters).
const MAX_EXAMPLES_CHARS: usize = 16_384;
const MAX_SETTINGS_CHARS: usize = 8_192;

fn invalid<T>(field: &str, message: &str) -> Result<T, AccountError> {
    Err(AccountError::Validation(
        [(field.to_string(), message.to_string())].into(),
    ))
}

fn check_jpeg(bytes: &[u8], field: &str, what: &str) -> Result<(), AccountError> {
    if bytes.len() > MAX_BYTES || !bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return invalid(field, &format!("The {what} must be a JPEG under 700 KB."));
    }
    Ok(())
}

/// The server's grade proposal.
#[derive(Debug, Clone, PartialEq)]
pub struct Proposal {
    pub settings: Value,
    pub notes: String,
    pub scene_hues: Vec<HslBand>,
    pub confidence: Map<String, Value>,
    pub requests_used: u32,
}

/// Asks the server to grade from a reference image and a contact sheet
/// (`POST desktop/grade`). Both images must already be JPEGs within the server's limits
/// (700 KB, 1024 px). `settings` is sent only when given (required by the server for `Review`).
pub fn extract(
    client: &Client,
    mode: Mode,
    reference_jpeg: &[u8],
    sheet_jpeg: &[u8],
    examples: &[&Look],
    settings: Option<&EditSettings>,
) -> Result<Proposal, AccountError> {
    check_jpeg(reference_jpeg, "reference", "reference picture")?;
    check_jpeg(sheet_jpeg, "sheet", "contact sheet")?;
    if examples.len() > MAX_EXAMPLES {
        return invalid("examples", "Use at most 16 example looks.");
    }
    if mode == Mode::Review && settings.is_none() {
        return invalid("settings", "A review needs the current edit settings.");
    }
    let jpeg = |bytes: &[u8], name: &'static str| {
        Part::bytes(bytes.to_vec())
            .file_name(name)
            .mime_str("image/jpeg")
            .expect("static mime")
    };
    let examples: Vec<Value> = examples
        .iter()
        .map(|l| json!({"id": l.id, "name": l.name, "settings": l.settings}))
        .collect();
    let examples = Value::Array(examples).to_string();
    if examples.chars().count() > MAX_EXAMPLES_CHARS {
        return invalid(
            "examples",
            "The example looks are too large to send; use fewer of them.",
        );
    }
    let settings = settings.map(|s| s.to_value().to_string());
    if settings
        .as_ref()
        .is_some_and(|s| s.chars().count() > MAX_SETTINGS_CHARS)
    {
        return invalid(
            "settings",
            "The current edit is too large to send for review (often too many spots); remove some and try again.",
        );
    }
    let mut form = Form::new()
        .text("mode", mode.as_str())
        .part("reference", jpeg(reference_jpeg, "reference.jpg"))
        .part("sheet", jpeg(sheet_jpeg, "sheet.jpg"))
        .text("examples", examples);
    if let Some(s) = settings {
        form = form.text("settings", s);
    }
    let long = client.long();
    let body = long.send(
        long.request(reqwest::Method::POST, "desktop/grade")
            .multipart(form),
    )?;
    let data = body.get("data").unwrap_or(&body);
    Ok(Proposal {
        settings: data.get("settings").cloned().unwrap_or(Value::Null),
        notes: data
            .get("notes")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        scene_hues: data
            .get("scene_hues")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| serde_json::from_value(v.clone()).ok())
                    .collect()
            })
            .unwrap_or_default(),
        confidence: data
            .get("confidence")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default(),
        requests_used: data
            .get("requests_used")
            .and_then(Value::as_u64)
            .map(|n| n as u32)
            .unwrap_or(1),
    })
}
