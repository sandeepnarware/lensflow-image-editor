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
    let mut form = Form::new()
        .text("mode", mode.as_str())
        .part("reference", jpeg(reference_jpeg, "reference.jpg"))
        .part("sheet", jpeg(sheet_jpeg, "sheet.jpg"))
        .text("examples", Value::Array(examples).to_string());
    if let Some(s) = settings {
        form = form.text("settings", s.to_value().to_string());
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
