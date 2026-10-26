use crate::{AccountError, Instance, LockReason};
use reqwest::blocking::{RequestBuilder, Response};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use url::Url;

struct Inner {
    http: reqwest::blocking::Client,
    base: Url,
    version: String,
    token: Option<String>,
    studio: Option<String>,
}

/// The LensFlow API client. Cheap to clone (an `Arc` inside); the `with_*` methods
/// return a copy carrying the extra header.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the token.
        f.debug_struct("Client")
            .field("base", &self.inner.base.as_str())
            .field("studio", &self.inner.studio)
            .field("signed_in", &self.inner.token.is_some())
            .finish()
    }
}

fn build_http(long: bool) -> reqwest::blocking::Client {
    let mut b = reqwest::blocking::Client::builder().connect_timeout(Duration::from_secs(20));
    b = if long {
        b.timeout(Duration::from_secs(120))
    } else {
        b.timeout(Duration::from_secs(30))
    };
    b.build().expect("http client")
}

impl Client {
    pub fn new(instance: Instance, version: &str) -> Client {
        let mut base = instance.base_url();
        if !base.path().ends_with('/') {
            let p = format!("{}/", base.path());
            base.set_path(&p);
        }
        Client {
            inner: Arc::new(Inner {
                http: build_http(false),
                base,
                version: version.to_string(),
                token: None,
                studio: None,
            }),
        }
    }

    fn derive(&self, f: impl FnOnce(&mut Inner)) -> Client {
        let mut inner = Inner {
            http: self.inner.http.clone(),
            base: self.inner.base.clone(),
            version: self.inner.version.clone(),
            token: self.inner.token.clone(),
            studio: self.inner.studio.clone(),
        };
        f(&mut inner);
        Client {
            inner: Arc::new(inner),
        }
    }

    pub fn with_token(&self, token: &str) -> Client {
        self.derive(|i| i.token = Some(token.to_string()))
    }

    pub fn with_studio(&self, slug: &str) -> Client {
        self.derive(|i| i.studio = Some(slug.to_string()))
    }

    /// The same client with the 120 s read timeout for grading and uploads.
    pub fn long(&self) -> Client {
        self.derive(|i| i.http = build_http(true))
    }

    pub fn base_url(&self) -> &Url {
        &self.inner.base
    }

    /// A request with every standard header set; `path` is relative to the API base.
    pub fn request(&self, method: reqwest::Method, path: &str) -> RequestBuilder {
        let url = self
            .inner
            .base
            .join(path.trim_start_matches('/'))
            .expect("api path");
        let mut r = self
            .inner
            .http
            .request(method, url)
            .header("Accept", "application/json")
            .header("X-LensFlow-Client", "desktop")
            .header(
                "User-Agent",
                format!(
                    "lensflow-ImageEditor/{} ({})",
                    self.inner.version,
                    std::env::consts::OS
                ),
            );
        if let Some(t) = &self.inner.token {
            r = r.bearer_auth(t);
        }
        if let Some(s) = &self.inner.studio {
            r = r.header("X-Studio", s);
        }
        r
    }

    /// [`Client::send`], and on an error answer also the AI requests it still cost (the
    /// body's `requests_used`; 0 when absent or nothing was answered).
    pub(crate) fn send_spent(&self, req: RequestBuilder) -> Result<Value, (AccountError, u32)> {
        let resp = req
            .send()
            .map_err(|e| (AccountError::Network(e.without_url().to_string()), 0))?;
        let status = resp.status().as_u16();
        let text = resp
            .text()
            .map_err(|e| (AccountError::Network(e.without_url().to_string()), 0))?;
        let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        let used = spent(&body);
        map_body(status, body).map_err(|e| (e, used))
    }

    /// Sends a request and maps the response to JSON or a typed error.
    pub fn send(&self, req: RequestBuilder) -> Result<Value, AccountError> {
        let resp = req
            .send()
            .map_err(|e| AccountError::Network(e.without_url().to_string()))?;
        map_response(resp)
    }

    pub fn get_json(&self, path: &str) -> Result<Value, AccountError> {
        self.send(self.request(reqwest::Method::GET, path))
    }

    pub fn post_json(&self, path: &str, body: &Value) -> Result<Value, AccountError> {
        self.send(self.request(reqwest::Method::POST, path).json(body))
    }
}

fn map_response(resp: Response) -> Result<Value, AccountError> {
    let status = resp.status().as_u16();
    let text = resp
        .text()
        .map_err(|e| AccountError::Network(e.without_url().to_string()))?;
    map_body(status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

/// An error answer's `requests_used` (the AI requests a failed grade still cost).
fn spent(body: &Value) -> u32 {
    body.get("requests_used")
        .and_then(Value::as_u64)
        .map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX))
}

fn map_body(status: u16, body: Value) -> Result<Value, AccountError> {
    if (200..300).contains(&status) {
        return Ok(body);
    }
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .filter(|m| !m.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("The server answered {status}."));
    let code = body.get("code").and_then(Value::as_str).unwrap_or("");
    Err(match (status, code) {
        (401, _) => AccountError::Unauthorized,
        (404, "studio_not_found") => AccountError::StudioNotFound,
        (403 | 402, "studio_suspended") => AccountError::Locked(LockReason::Suspended, message),
        (403 | 402, "desktop_editor_disabled") => {
            AccountError::Locked(LockReason::NotEnabled, message)
        }
        (403 | 402, "subscription_paused") => AccountError::Locked(LockReason::Paused, message),
        (402, "subscription_required") => AccountError::ReadOnly(message),
        (422, "plan_limit") => AccountError::PlanLimit(message),
        (422, _) => AccountError::Validation(validation_map(&body, message)),
        (429, _) => AccountError::RateLimited,
        _ => AccountError::Server(status, message),
    })
}

fn validation_map(body: &Value, message: String) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    if let Some(errors) = body.get("errors").and_then(Value::as_object) {
        for (field, msgs) in errors {
            let first = msgs
                .as_array()
                .and_then(|a| a.first())
                .and_then(Value::as_str);
            if let Some(m) = first.or_else(|| msgs.as_str()) {
                map.insert(field.clone(), m.to_string());
            }
        }
    }
    if map.is_empty() {
        map.insert("message".to_string(), message);
    }
    map
}
