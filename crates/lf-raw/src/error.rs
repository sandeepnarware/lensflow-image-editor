use std::path::PathBuf;

/// Why a file could not be decoded. Every message names the file.
#[derive(Debug, thiserror::Error)]
pub enum RawError {
    #[error("{}: this camera or file format is not supported ({reason})", path.display())]
    Unsupported { path: PathBuf, reason: String },
    #[error("{}: could not be read ({source})", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{}: the file looks damaged and could not be decoded ({reason})", path.display())]
    Corrupt { path: PathBuf, reason: String },
    #[error("{} is not a photo this app can open.", path.display())]
    NotAnImage { path: PathBuf },
    #[error("the photo has no pixels inside its crop")]
    Empty,
}
