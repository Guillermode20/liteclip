//! Capture subsystem errors (video/audio backends).

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("{0}")]
    Msg(String),
}

pub type CaptureResult<T> = std::result::Result<T, CaptureError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msg_error_display() {
        let err = CaptureError::Msg("DXGI access lost".to_string());
        assert_eq!(format!("{}", err), "DXGI access lost");
    }

    #[test]
    fn capture_result_alias() {
        fn returns_result() -> CaptureResult<String> {
            Ok("captured".to_string())
        }
        assert_eq!(returns_result().unwrap(), "captured");
    }
}
