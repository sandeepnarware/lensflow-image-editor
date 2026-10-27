//! Export to a LensFlow gallery: the studio's galleries and their sections, and the upload of
//! a finished JPEG as a new photo, through the same resumable upload sessions the web's
//! uploader uses (`galleries/{g}/uploads`).
//!
//! An upload starts a session (`filename`, `bytes`, `mime`, `sha1`, `section_id`), sends the
//! file in the session's chunks, and completes it. The session is either chunked through the
//! API (`PUT uploads/{id}/chunks/{n}`) or, when the gallery's storage gives one, direct: a
//! short-lived block-blob link (`direct.url`) the blocks are put to (`&comp=block`, ids as
//! the web makes them) and then joined (`&comp=blocklist`), before the same complete. The
//! server answers a file it already has with 409 and `duplicate_photo_id`.
//!
//! Per the editor's design, the rendered JPEG goes up as an ordinary new photo: the server's
//! `edit` record needs a photo already in the gallery, so a desktop photo's settings are not
//! attached.

use crate::client::map_body;
use crate::{AccountError, Client};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};

/// A gallery the studio can upload to.
#[derive(Debug, Clone, PartialEq)]
pub struct Gallery {
    pub id: i64,
    pub title: String,
    /// `YYYY-MM-DD`, when the gallery has one.
    pub event_date: Option<String>,
    pub status: String,
}

/// A section of a gallery.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub id: i64,
    pub name: String,
}

/// What an upload came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Uploaded {
    /// A new photo.
    Photo(i64),
    /// The gallery already holds this file (that photo, when the server says which).
    Duplicate(Option<i64>),
}

fn bad(what: &str) -> AccountError {
    AccountError::Server(200, format!("Unexpected {what} response."))
}

/// The studio's galleries that are not archived, most recently changed first (up to 100;
/// `search` matches a title or a customer).
pub fn list(client: &Client, search: Option<&str>) -> Result<Vec<Gallery>, AccountError> {
    let mut path = "galleries?status=active&per_page=100&sort=recent".to_owned();
    if let Some(s) = search.map(str::trim).filter(|s| !s.is_empty()) {
        path.push_str("&search=");
        path.extend(url::form_urlencoded::byte_serialize(s.as_bytes()));
    }
    let body = client.get_json(&path)?;
    body.get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("gallery list"))?
        .iter()
        .map(|g| {
            Ok(Gallery {
                id: g
                    .get("id")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| bad("gallery"))?,
                title: g
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("Untitled gallery")
                    .to_owned(),
                event_date: g
                    .get("event_date")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                status: g
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            })
        })
        .collect()
}

/// A gallery's sections, in its order.
pub fn sections(client: &Client, gallery: i64) -> Result<Vec<Section>, AccountError> {
    let body = client.get_json(&format!("galleries/{gallery}/sections"))?;
    body.get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("section list"))?
        .iter()
        .map(|s| {
            Ok(Section {
                id: s
                    .get("id")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| bad("section"))?,
                name: s
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            })
        })
        .collect()
}

/// The web uploader's block id: `btoa("block-000123")`. Azure needs every id of a blob to be
/// the same length, hence the padding.
pub fn block_id(index: usize) -> String {
    base64(format!("block-{index:06}").as_bytes())
}

fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The duplicate a 409 names, if it is one (else the 409 is a stopped or expired session).
fn duplicate(status: u16, body: &Value) -> Option<Uploaded> {
    if status != 409 {
        return None;
    }
    let id = body.get("duplicate_photo_id").and_then(Value::as_i64);
    let flagged = body.get("duplicate").and_then(Value::as_bool) == Some(true);
    (id.is_some() || flagged).then_some(Uploaded::Duplicate(id))
}

/// Sends `req` and reads the answer, telling a duplicate apart from other errors.
fn send(req: reqwest::blocking::RequestBuilder) -> Result<Result<Value, Uploaded>, AccountError> {
    let resp = req
        .send()
        .map_err(|e| AccountError::Network(e.without_url().to_string()))?;
    let status = resp.status().as_u16();
    let text = resp
        .text()
        .map_err(|e| AccountError::Network(e.without_url().to_string()))?;
    let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if let Some(dup) = duplicate(status, &body) {
        return Ok(Err(dup));
    }
    map_body(status, body).map(Ok)
}

/// Uploads `jpeg` to `gallery` (into `section`, if given) as `filename`. `progress` is told
/// how many bytes have gone up after each chunk. The client should be [`Client::long`]: a
/// chunk of several megabytes takes a while on a slow line.
pub fn upload(
    client: &Client,
    gallery: i64,
    section: Option<i64>,
    filename: &str,
    jpeg: &[u8],
    progress: &mut dyn FnMut(usize),
) -> Result<Uploaded, AccountError> {
    let sha1 = hex::encode(Sha1::digest(jpeg));
    let base = format!("galleries/{gallery}/uploads");
    let mut start = json!({
        "filename": filename,
        "bytes": jpeg.len(),
        "mime": "image/jpeg",
        "sha1": sha1,
    });
    if let Some(section) = section {
        start["section_id"] = json!(section);
    }
    let session = match send(client.request(reqwest::Method::POST, &base).json(&start))? {
        Ok(body) => body.get("data").cloned().unwrap_or(body),
        Err(dup) => return Ok(dup),
    };
    let id = session
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("upload"))?
        .to_owned();
    let chunk = session
        .get("chunk_size")
        .and_then(Value::as_u64)
        .filter(|&n| n > 0)
        .ok_or_else(|| bad("upload"))? as usize;
    let direct = session
        .get("direct")
        .and_then(|d| d.get("url"))
        .and_then(Value::as_str)
        .map(str::to_owned);

    let mut sent = 0;
    match &direct {
        Some(url) => {
            // Straight to storage: the link holds its own permission, so no API headers.
            let mut ids = Vec::new();
            for (i, part) in jpeg.chunks(chunk).enumerate() {
                let block = block_id(i);
                let resp = client
                    .http()
                    .put(format!("{url}&comp=block&blockid={}", urlencode(&block)))
                    .header("Content-Type", "application/octet-stream")
                    .body(part.to_vec())
                    .send()
                    .map_err(|e| AccountError::Network(e.without_url().to_string()))?;
                if !resp.status().is_success() {
                    return Err(AccountError::Server(
                        resp.status().as_u16(),
                        "The photo could not be sent to the gallery's storage.".into(),
                    ));
                }
                ids.push(block);
                sent += part.len();
                progress(sent);
            }
            let list: String = ids
                .iter()
                .map(|b| format!("<Latest>{b}</Latest>"))
                .collect();
            let resp = client
                .http()
                .put(format!("{url}&comp=blocklist"))
                .header("Content-Type", "application/xml")
                .header("x-ms-blob-content-type", "image/jpeg")
                .body(format!(
                    r#"<?xml version="1.0" encoding="utf-8"?><BlockList>{list}</BlockList>"#
                ))
                .send()
                .map_err(|e| AccountError::Network(e.without_url().to_string()))?;
            if !resp.status().is_success() {
                return Err(AccountError::Server(
                    resp.status().as_u16(),
                    "The gallery's storage did not accept the photo.".into(),
                ));
            }
        }
        None => {
            for (i, part) in jpeg.chunks(chunk).enumerate() {
                let req = client
                    .request(reqwest::Method::PUT, &format!("{base}/{id}/chunks/{i}"))
                    .header("Content-Type", "application/octet-stream")
                    .body(part.to_vec());
                if let Err(dup) = send(req)? {
                    return Ok(dup);
                }
                sent += part.len();
                progress(sent);
            }
        }
    }
    let done = match send(
        client
            .request(reqwest::Method::POST, &format!("{base}/{id}/complete"))
            .json(&json!({ "sha1": sha1 })),
    )? {
        Ok(body) => body,
        Err(dup) => return Ok(dup),
    };
    let photo = done
        .get("data")
        .and_then(|d| d.get("photo"))
        .and_then(|p| p.get("id"))
        .and_then(Value::as_i64)
        .ok_or_else(|| bad("upload"))?;
    Ok(Uploaded::Photo(photo))
}

/// Percent-encodes a base64 block id for a query string (`+`, `/`, `=`).
fn urlencode(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '+' => "%2B".to_owned(),
            '/' => "%2F".to_owned(),
            '=' => "%3D".to_owned(),
            c => c.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_ids_are_the_web_uploaders() {
        // btoa('block-000000') and btoa('block-000123') in a browser.
        assert_eq!(block_id(0), "YmxvY2stMDAwMDAw");
        assert_eq!(block_id(123), "YmxvY2stMDAwMTIz");
        assert_eq!(base64(b"a"), "YQ==");
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"abc"), "YWJj");
        assert_eq!(urlencode("a+b/c="), "a%2Bb%2Fc%3D");
    }

    #[test]
    fn a_409_is_a_duplicate_only_when_it_says_so() {
        assert_eq!(
            duplicate(409, &json!({"duplicate_photo_id": 7})),
            Some(Uploaded::Duplicate(Some(7)))
        );
        assert_eq!(
            duplicate(409, &json!({"duplicate": true})),
            Some(Uploaded::Duplicate(None))
        );
        assert_eq!(duplicate(409, &json!({"message": "expired"})), None);
        assert_eq!(duplicate(422, &json!({"duplicate_photo_id": 7})), None);
    }
}
