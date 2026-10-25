use std::collections::BTreeMap;

use url::Url;

use crate::AccountError;

/// Which LensFlow API the editor talks to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Instance {
    Prod,
    Dev,
    /// Built with [`Instance::custom`], which refuses plain `http` except on this machine.
    Custom(CustomUrl),
}

/// A checked custom base URL (`https`, or `http` on `localhost` / `127.0.0.1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomUrl(Url);

impl CustomUrl {
    pub fn url(&self) -> &Url {
        &self.0
    }
}

impl Instance {
    /// A custom API base URL. The sign-in token travels with every request, so anything but
    /// `https` is refused, except `http` to `localhost` or `127.0.0.1` (local development
    /// and tests).
    pub fn custom(url: Url) -> Result<Instance, AccountError> {
        let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        let ok = url.scheme() == "https" || (url.scheme() == "http" && local);
        if !ok {
            return Err(AccountError::Validation(BTreeMap::from([(
                "url".to_string(),
                "The server address must start with https:// (plain http is allowed only for localhost)."
                    .to_string(),
            )])));
        }
        Ok(Instance::Custom(CustomUrl(url)))
    }

    pub fn base_url(&self) -> Url {
        match self {
            Instance::Prod => Url::parse("https://lensflow.in/api").expect("static url"),
            Instance::Dev => Url::parse("https://dev.lensflow.in/api").expect("static url"),
            Instance::Custom(url) => url.0.clone(),
        }
    }
}
