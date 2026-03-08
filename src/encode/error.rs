use thiserror::Error;

#[derive(Debug, Error)]
pub enum EncodeError {
    #[error("Codec not found: {0}")]
    CodecNotFound(String),

    #[error("Encoder initialization failed: {0}")]
    InitializationFailed(String),

    #[error("Encoding failed: {0}")]
    EncodingFailed(String),

    #[error("Hardware encoder error: {0}")]
    HardwareError(String),
}
