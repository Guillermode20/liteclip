//! Buffer subsystem errors.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum BufferError {
    #[error("{0}")]
    Msg(String),
}

pub type BufferResult<T> = std::result::Result<T, BufferError>;
