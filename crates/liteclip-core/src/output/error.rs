//! Output / muxing subsystem errors.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum OutputError {
    #[error("{0}")]
    Msg(String),
}

pub type OutputResult<T> = std::result::Result<T, OutputError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msg_error_display() {
        let err = OutputError::Msg("muxer failed".to_string());
        assert_eq!(format!("{}", err), "muxer failed");
    }

    #[test]
    fn output_result_alias() {
        fn returns_result() -> OutputResult<String> {
            Ok("output".to_string())
        }
        assert_eq!(returns_result().unwrap(), "output");
    }
}
