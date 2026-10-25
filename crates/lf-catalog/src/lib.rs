//! The catalogue: a SQLite database per catalogue folder, `<photo>.lensflow.json` sidecars, edit
//! history and named versions. This crate never touches pixels.

pub mod db;
pub mod edits;
pub mod import;
pub mod model;
pub mod sidecar;
pub mod thumbs;
pub mod versions;

pub use db::{Catalog, CatalogError};
pub use model::{NewPhoto, Reconciled, Source, Version};
pub use sidecar::Sidecar;
