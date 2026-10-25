//! lensflow-ImageEditor, the desktop application. `main.rs` opens the window; everything
//! else lives here so the unit tests and later screens share one crate.

pub mod app;
pub mod gpu;
pub mod jobs;
pub mod logging;
pub mod prefs;
pub mod theme;
pub mod toast;
pub mod version;
