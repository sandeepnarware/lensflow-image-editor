//! Test helpers (the `test-support` feature).
//!
//! WARP (Windows' software adapter) crashes when one process opens several devices at once,
//! which parallel tests do if each opens its own [`Gpu`]. Every GPU test in the workspace
//! therefore shares one device per test binary through [`shared_gpu`]. Tests that change
//! device-wide state (the uncaptured-error handler, destroying the device) open their own
//! device in a test binary of their own instead.

use std::sync::{Arc, OnceLock};

use crate::error::PipelineError;
use crate::gpu::Gpu;

/// The test binary's one device, or `None` when the machine has no adapter at all. Any
/// other failure to open it panics.
pub fn try_shared_gpu() -> Option<Arc<Gpu>> {
    static GPU: OnceLock<Option<Arc<Gpu>>> = OnceLock::new();
    GPU.get_or_init(|| match Gpu::new() {
        Ok(g) => Some(g),
        Err(PipelineError::NoAdapter) => None,
        Err(e) => panic!("{e}"),
    })
    .clone()
}

/// The test binary's one device; panics when there is none.
pub fn shared_gpu() -> Arc<Gpu> {
    try_shared_gpu().expect("no GPU adapter")
}
