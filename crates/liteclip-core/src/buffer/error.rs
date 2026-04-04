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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msg_error_display() {
        let err = BufferError::Msg("buffer full".to_string());
        assert_eq!(format!("{}", err), "buffer full");
    }

    #[test]
    fn snapshot_limit_exceeded_display() {
        let err = BufferError::SnapshotLimitExceeded {
            current: 512_000_000,
            limit: 256_000_000,
        };
        let msg = format!("{}", err);
        assert!(msg.contains("512000000"));
        assert!(msg.contains("256000000"));
    }

    #[test]
    fn buffer_result_alias() {
        fn returns_result() -> BufferResult<String> {
            Ok("ok".to_string())
        }
        assert_eq!(returns_result().unwrap(), "ok");
    }
}
