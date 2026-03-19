//! Capture subsystem errors (video/audio backends).

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("{0}")]
    Msg(String),
}

pub type CaptureResult<T> = std::result::Result<T, CaptureError>;
