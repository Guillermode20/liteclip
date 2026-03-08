use thiserror::Error;

#[derive(Debug, Error)]
pub enum LiteClipError {
    #[error("Configuration error: {0}")]
    Config(#[from] ConfigError),

    #[error("Capture error: {0}")]
    Capture(#[from] crate::capture::CaptureError),

    #[error("Encoding error: {0}")]
    Encode(#[from] crate::encode::EncodeError),

    #[error("Buffer error: {0}")]
    Buffer(#[from] crate::buffer::BufferError),

    #[error("Output error: {0}")]
    Output(#[from] crate::output::OutputError),
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Configuration path error: {0}")]
    Path(String),

    #[error("Configuration serialization error: {0}")]
    Serialization(String),

    #[error("Configuration I/O error: {0}")]
    Io(String),
}
