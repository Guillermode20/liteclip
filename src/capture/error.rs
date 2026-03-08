use thiserror::Error;

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("DXGI error: {0}")]
    Dxgi(String),

    #[error("Audio error: {0}")]
    Audio(String),

    #[error("Capture not initialized")]
    NotInitialized,
}
