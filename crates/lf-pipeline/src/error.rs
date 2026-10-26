//! Errors the pipeline reports to the user.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PipelineError {
    #[error(
        "No graphics device found. lensflow-ImageEditor needs a GPU with Vulkan, Metal or DirectX 12."
    )]
    NoAdapter,
    #[error("Could not open the graphics device: {0}")]
    DeviceRequest(String),
    #[error(
        "This photo is {width}×{height} pixels; this graphics device handles at most {limit} pixels on a side."
    )]
    TooLarge { width: u32, height: u32, limit: u32 },
    #[error("The photo has no pixels.")]
    EmptyImage,
    #[error(
        "The graphics device stopped working (it was reset or removed). Restart to keep editing."
    )]
    DeviceLost,
    #[error("The graphics device reported an error: {0}")]
    Gpu(String),
    #[error("Not enough graphics memory for this photo.")]
    OutOfMemory,
    #[error("Could not read the image back from the graphics device: {0}")]
    Readback(String),
    #[error("The graphics device did not answer within {0} seconds.")]
    Timeout(u64),
    #[error("The image texture is {got}; the renderer needs {expected}.")]
    WrongFormat { expected: String, got: String },
    #[error("A {width}×{height} picture needs {expected} values; this one has {got}.")]
    BadFrame {
        width: u32,
        height: u32,
        expected: usize,
        got: usize,
    },
}
