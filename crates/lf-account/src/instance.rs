use url::Url;

/// Which LensFlow API the editor talks to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Instance {
    Prod,
    Dev,
    Custom(Url),
}

impl Instance {
    pub fn base_url(&self) -> Url {
        match self {
            Instance::Prod => Url::parse("https://lensflow.in/api").expect("static url"),
            Instance::Dev => Url::parse("https://dev.lensflow.in/api").expect("static url"),
            Instance::Custom(url) => url.clone(),
        }
    }
}
