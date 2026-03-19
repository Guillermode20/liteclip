//! Output / muxing subsystem errors.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum OutputError {
    #[error("{0}")]
    Msg(String),
}

pub type OutputResult<T> = std::result::Result<T, OutputError>;
