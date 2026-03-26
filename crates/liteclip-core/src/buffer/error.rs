//! Buffer subsystem errors.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum BufferError {
    #[error("{0}")]
    Msg(String),
    /// Snapshot rejected because too many snapshots are already in-flight,
    /// which would cause unbounded memory growth.
    #[error("Snapshot rejected: outstanding snapshot bytes ({current}) exceeds limit ({limit})")]
    SnapshotLimitExceeded { current: usize, limit: usize },
}

pub type BufferResult<T> = std::result::Result<T, BufferError>;
