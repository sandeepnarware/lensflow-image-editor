//! LensFlow account access for the desktop editor: a blocking HTTP client, sign-in,
//! the token store, and the entitlement gate with its offline grace.

pub mod auth;
pub mod client;
pub mod entitlement;
pub mod error;
pub mod grade;
pub mod instance;
pub mod presets;
pub mod token_store;

pub use client::Client;
pub use entitlement::{DownloadLink, download_link};
pub use error::{AccountError, LockReason};
pub use instance::Instance;
pub use token_store::{TokenStore, handle_unauthorized};
