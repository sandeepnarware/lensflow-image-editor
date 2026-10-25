//! lensflow-ImageEditor, the desktop application. `main.rs` opens the window; everything
//! else lives here so the unit tests and later screens share one crate.

pub mod account;
pub mod app;
pub mod gpu;
pub mod jobs;
pub mod logging;
pub mod prefs;
pub mod screens;
pub mod session;
pub mod theme;
pub mod toast;
pub mod update;
pub mod version;
