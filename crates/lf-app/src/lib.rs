//! lensflow-ImageEditor, the desktop application. `main.rs` opens the window; everything
//! else lives here so the unit tests and later screens share one crate.

pub mod account;
pub mod app;
pub mod catalog_ext;
pub mod clipboard;
pub mod develop;
pub mod device_reset;
pub mod dialogs;
pub mod gpu;
pub mod jobs;
pub mod library;
pub mod logging;
pub mod platform;
pub mod prefs;
pub mod presets_sync;
pub mod screens;
pub mod session;
pub mod session_flow;
pub mod theme;
pub mod thumbs_ui;
pub mod toast;
pub mod update;
pub mod version;
