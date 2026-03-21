use thiserror::Error;

/// Core error type for the LiteClip recording engine.
#[derive(Debug, Error)]
pub enum LiteClipError {
    #[error("Capture pipeline failed: {0}")]
    CaptureError(String),

    #[error("Encoding or FFmpeg backend failed: {0}")]
    EncodeError(String),

    #[error("Configuration or path error: {0}")]
    ConfigError(String),

    #[error("Internal application state error: {0}")]
    StateError(String),

    #[error(transparent)]
    Unknown(#[from] anyhow::Error),
}

/// A specialized Result type for LiteClip operations.
pub type Result<T, E = LiteClipError> = std::result::Result<T, E>;
