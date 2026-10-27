//! The catalogue: a SQLite database per catalogue folder, `<photo>.lensflow.json` sidecars, edit
//! history and named versions. This crate never touches pixels.

pub mod cull;
pub mod db;
pub mod edits;
pub mod exif_embed;
pub mod export;
pub mod finish;
pub mod import;
pub mod model;
pub mod presets;
pub mod sidecar;
pub mod thumbs;
pub mod versions;

pub use db::{Catalog, CatalogError, Folder, PhotoRow};
pub use model::{NewPhoto, Reconciled, Source, Version};
pub use sidecar::Sidecar;
