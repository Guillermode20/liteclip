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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_error_display() {
        let err = LiteClipError::CaptureError("DXGI lost".to_string());
        assert_eq!(format!("{}", err), "Capture pipeline failed: DXGI lost");
    }

    #[test]
    fn encode_error_display() {
        let err = LiteClipError::EncodeError("NVENC unavailable".to_string());
        assert_eq!(
            format!("{}", err),
            "Encoding or FFmpeg backend failed: NVENC unavailable"
        );
    }

    #[test]
    fn config_error_display() {
        let err = LiteClipError::ConfigError("File not found".to_string());
        assert_eq!(
            format!("{}", err),
            "Configuration or path error: File not found"
        );
    }

    #[test]
    fn state_error_display() {
        let err = LiteClipError::StateError("Pipeline stopped".to_string());
        assert_eq!(
            format!("{}", err),
            "Internal application state error: Pipeline stopped"
        );
    }

    #[test]
    fn from_anyhow_error() {
        let anyhow_err = anyhow::anyhow!("something went wrong");
        let err: LiteClipError = anyhow_err.into();
        match err {
            LiteClipError::Unknown(e) => {
                assert!(e.to_string().contains("something went wrong"));
            }
            _ => panic!("Expected Unknown variant"),
        }
    }

    #[test]
    fn error_is_send_sync() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<LiteClipError>();
        assert_sync::<LiteClipError>();
    }

    #[test]
    fn result_alias_works() {
        fn returns_result() -> Result<String> {
            Ok("success".to_string())
        }
        assert_eq!(returns_result().unwrap(), "success");
    }
}
