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
    #[error("Not enough graphics memory for this photo.")]
    OutOfMemory,
    #[error("Could not read the image back from the graphics device: {0}")]
    Readback(String),
}
