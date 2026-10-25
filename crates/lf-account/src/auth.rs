use crate::{AccountError, Client};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Clone, Deserialize)]
pub struct StudioSummary {
    pub id: u64,
    pub name: String,
    pub slug: String,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub plan_features: Vec<String>,
    #[serde(default)]
    pub subscription_paused: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub id: u64,
    pub name: String,
    pub email: String,
    #[serde(default)]
    pub is_platform_admin: bool,
    #[serde(default)]
    pub studios: Vec<StudioSummary>,
}

#[derive(Clone)]
pub struct SignedIn {
    pub token: String,
    pub user: User,
}

fn parse<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, AccountError> {
    serde_json::from_value(v)
        .map_err(|e| AccountError::Server(200, format!("unexpected reply: {e}")))
}

/// `POST auth/login`. A `TwoFactorRequired` error means ask for the code and call again.
pub fn sign_in(
    client: &Client,
    email: &str,
    password: &str,
    two_factor_code: Option<&str>,
    device_name: &str,
) -> Result<SignedIn, AccountError> {
    let mut body = json!({"email": email, "password": password, "device_name": device_name});
    if let Some(code) = two_factor_code.filter(|c| !c.is_empty()) {
        body["two_factor_code"] = json!(code);
    }
    let reply = client.post_json("auth/login", &body)?;
    if reply.get("two_factor_required").and_then(Value::as_bool) == Some(true) {
        return Err(AccountError::TwoFactorRequired);
    }
    let token = reply
        .get("token")
        .and_then(Value::as_str)
        .ok_or_else(|| AccountError::Server(200, "the reply had no token".into()))?
        .to_string();
    let user = parse(reply.get("user").cloned().unwrap_or(Value::Null))?;
    Ok(SignedIn { token, user })
}

/// `GET auth/me`.
pub fn me(client: &Client) -> Result<User, AccountError> {
    let reply = client.get_json("auth/me")?;
    parse(reply.get("data").cloned().unwrap_or(reply))
}

/// `POST auth/logout`; failures are ignored (the token is dropped locally either way).
pub fn sign_out(client: &Client) {
    let _ = client.post_json("auth/logout", &json!({}));
}

impl std::fmt::Debug for SignedIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The bearer token never prints.
        f.debug_struct("SignedIn")
            .field("token", &"<redacted>")
            .field("user", &self.user)
            .finish()
    }
}
