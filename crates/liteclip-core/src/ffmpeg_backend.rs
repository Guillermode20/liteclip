//! FFmpeg integration mode: linked **SDK/DLLs** (`ffmpeg` feature).
//!
//! The `ffmpeg` feature must be enabled for recording. Use [`compiled_backend_kind`] and
//! [`validate_runtime`] before starting the pipeline.

use thiserror::Error;

/// Which FFmpeg backend this build uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfmpegBackendKind {
    /// Linked `ffmpeg-next` / FFmpeg shared libraries.
    Sdk,
}

/// Returns the backend selected at **compile time** by Cargo features.
#[must_use]
pub fn compiled_backend_kind() -> Option<FfmpegBackendKind> {
    if cfg!(feature = "ffmpeg") {
        Some(FfmpegBackendKind::Sdk)
    } else {
        None
    }
}

/// Validate runtime dependencies for the compiled backend (call once at startup).
pub fn validate_runtime() -> Result<(), FfmpegRuntimeError> {
    match compiled_backend_kind() {
        Some(FfmpegBackendKind::Sdk) => validate_sdk_runtime(),
        None => Ok(()),
    }
}

/// No-op for SDK mode if you already called [`crate::encode::init_ffmpeg`].
/// Prefer calling [`validate_runtime`] after `init_ffmpeg` so validation does not initialize twice.
pub fn validate_sdk_runtime() -> Result<(), FfmpegRuntimeError> {
    #[cfg(feature = "ffmpeg")]
    {
        Ok(())
    }
    #[cfg(not(feature = "ffmpeg"))]
    {
        Ok(())
    }
}


/// User-facing validation failures for embedders (no bundling; users install FFmpeg themselves).
#[derive(Debug, Error)]
pub enum FfmpegRuntimeError {
    #[error(
        "ffmpeg SDK not properly initialized; check FFmpeg DLLs are present"
    )]
    FfmpegSdkMissing,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiled_kind_matches_feature_matrix() {
        let k = compiled_backend_kind();
        #[cfg(feature = "ffmpeg")]
        assert_eq!(k, Some(FfmpegBackendKind::Sdk));
        #[cfg(not(feature = "ffmpeg"))]
        assert_eq!(k, None);
    }

    #[cfg(not(feature = "ffmpeg"))]
    #[test]
    fn validate_runtime_noops_without_encoder_backend() {
        assert!(validate_runtime().is_ok());
    }

    #[cfg(feature = "ffmpeg")]
    #[test]
    fn validate_sdk_runtime_succeeds_after_compile() {
        assert!(validate_sdk_runtime().is_ok());
        assert!(validate_runtime().is_ok());
    }
}
