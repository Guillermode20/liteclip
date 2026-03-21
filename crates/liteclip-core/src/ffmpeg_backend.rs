//! FFmpeg integration mode: linked **SDK/DLLs** (`ffmpeg` feature) vs **`ffmpeg.exe` subprocess** (`ffmpeg-cli`).
//!
//! Exactly one of those features must be enabled for recording; they are mutually exclusive.
//! Use [`compiled_backend_kind`] and [`validate_runtime`] before starting the pipeline.

use std::path::PathBuf;
#[cfg(feature = "ffmpeg-cli")]
use std::process::Command;
use thiserror::Error;

#[cfg(all(target_os = "windows", feature = "ffmpeg-cli"))]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Which FFmpeg backend this build uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfmpegBackendKind {
    /// Linked `ffmpeg-next` / FFmpeg shared libraries (recommended).
    Sdk,
    /// External `ffmpeg.exe` / `ffprobe` only (no `ffmpeg-next` link).
    Cli,
}

/// Returns the backend selected at **compile time** by Cargo features.
#[must_use]
pub fn compiled_backend_kind() -> Option<FfmpegBackendKind> {
    if cfg!(feature = "ffmpeg") {
        Some(FfmpegBackendKind::Sdk)
    } else if cfg!(feature = "ffmpeg-cli") {
        Some(FfmpegBackendKind::Cli)
    } else {
        None
    }
}

/// Validate runtime dependencies for the compiled backend (call once at startup).
pub fn validate_runtime() -> Result<(), FfmpegRuntimeError> {
    match compiled_backend_kind() {
        Some(FfmpegBackendKind::Sdk) => validate_sdk_runtime(),
        Some(FfmpegBackendKind::Cli) => validate_cli_runtime(),
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

/// Ensure `ffmpeg` and `ffprobe` can be executed (CLI mode).
pub fn validate_cli_runtime() -> Result<(), FfmpegRuntimeError> {
    #[cfg(feature = "ffmpeg-cli")]
    {
        let ffmpeg = crate::runtime::resolve_ffmpeg_executable();
        if !ffmpeg_looks_callable(&ffmpeg) {
            return Err(FfmpegRuntimeError::FfmpegExecutableMissing {
                path: ffmpeg.clone(),
            });
        }
        let ffprobe = ffprobe_path(&ffmpeg);
        if !ffmpeg_looks_callable(&ffprobe) {
            return Err(FfmpegRuntimeError::FfprobeMissing { path: ffprobe });
        }
        smoke_test_ffmpeg(&ffmpeg)?;
        Ok(())
    }
    #[cfg(not(feature = "ffmpeg-cli"))]
    {
        Ok(())
    }
}

#[cfg(feature = "ffmpeg-cli")]
fn ffmpeg_looks_callable(path: &std::path::Path) -> bool {
    if path.exists() {
        return true;
    }
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case("ffmpeg") || n.eq_ignore_ascii_case("ffprobe"))
}

#[cfg(feature = "ffmpeg-cli")]
fn ffprobe_path(ffmpeg: &std::path::Path) -> PathBuf {
    ffmpeg.with_file_name(if cfg!(windows) {
        "ffprobe.exe"
    } else {
        "ffprobe"
    })
}

#[cfg(feature = "ffmpeg-cli")]
fn smoke_test_ffmpeg(ffmpeg: &std::path::Path) -> Result<(), FfmpegRuntimeError> {
    let mut cmd = Command::new(ffmpeg);
    cmd.arg("-hide_banner")
        .arg("-version")
        .stdin(std::process::Stdio::null());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd.output().map_err(|e| FfmpegRuntimeError::FfmpegSpawn {
        path: ffmpeg.to_path_buf(),
        source: e,
    })?;
    if !out.status.success() {
        return Err(FfmpegRuntimeError::FfmpegVersionCheckFailed {
            path: ffmpeg.to_path_buf(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    Ok(())
}

/// User-facing validation failures for embedders (no bundling; users install FFmpeg themselves).
#[derive(Debug, Error)]
pub enum FfmpegRuntimeError {
    #[error(
        "ffmpeg CLI not found or not runnable at {path:?}; set {} or place ffmpeg.exe next to your app",
        crate::runtime::FFMPEG_ENV
    )]
    FfmpegExecutableMissing { path: PathBuf },
    #[error("ffprobe not found next to ffmpeg or on PATH (expected at {path:?})")]
    FfprobeMissing { path: PathBuf },
    #[error("failed to run ffmpeg at {path:?}: {source}")]
    FfmpegSpawn {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("ffmpeg -version failed for {path:?}: {stderr}")]
    FfmpegVersionCheckFailed { path: PathBuf, stderr: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiled_kind_matches_feature_matrix() {
        let k = compiled_backend_kind();
        #[cfg(all(feature = "ffmpeg", not(feature = "ffmpeg-cli")))]
        assert_eq!(k, Some(FfmpegBackendKind::Sdk));
        #[cfg(all(feature = "ffmpeg-cli", not(feature = "ffmpeg")))]
        assert_eq!(k, Some(FfmpegBackendKind::Cli));
        #[cfg(not(any(feature = "ffmpeg", feature = "ffmpeg-cli")))]
        assert_eq!(k, None);
    }

    #[cfg(not(any(feature = "ffmpeg", feature = "ffmpeg-cli")))]
    #[test]
    fn validate_runtime_noops_without_encoder_backend() {
        assert!(validate_runtime().is_ok());
    }

    #[cfg(all(feature = "ffmpeg", not(feature = "ffmpeg-cli")))]
    #[test]
    fn validate_sdk_runtime_succeeds_after_compile() {
        assert!(validate_sdk_runtime().is_ok());
        assert!(validate_runtime().is_ok());
    }

    #[test]
    fn ffmpeg_runtime_error_messages_mention_remediation() {
        let missing = FfmpegRuntimeError::FfmpegExecutableMissing {
            path: PathBuf::from(r"C:\nope\ffmpeg.exe"),
        };
        assert!(
            missing.to_string().contains(crate::runtime::FFMPEG_ENV),
            "{missing}"
        );

        let ffprobe = FfmpegRuntimeError::FfprobeMissing {
            path: PathBuf::from(r"C:\bin\ffprobe.exe"),
        };
        assert!(
            ffprobe.to_string().to_lowercase().contains("ffprobe"),
            "{ffprobe}"
        );

        let spawn = FfmpegRuntimeError::FfmpegSpawn {
            path: PathBuf::from("ffmpeg"),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "not found"),
        };
        assert!(
            spawn.to_string().contains("failed to run ffmpeg"),
            "{spawn}"
        );

        let ver = FfmpegRuntimeError::FfmpegVersionCheckFailed {
            path: PathBuf::from("ffmpeg"),
            stderr: "bad".into(),
        };
        assert!(ver.to_string().contains("-version"), "{ver}");
    }
}
