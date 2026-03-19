//! Typed errors for the encoding subsystem.

use thiserror::Error;

/// Failure modes for encoder resolution, FFmpeg, and I/O used by the encode pipeline.
#[derive(Debug, Error)]
pub enum EncodeError {
    /// Auto encoder was selected but no NVENC / AMF / QSV was detected.
    #[error("auto encoder selection could not find NVENC, AMF, or QSV on this system")]
    NoHardwareForAuto,

    /// Requested hardware encoder is missing from the FFmpeg build or runtime.
    #[error(
        "selected encoder {encoder:?} is not available in the current FFmpeg/runtime environment"
    )]
    EncoderUnavailable {
        encoder: crate::config::EncoderType,
    },

    #[error("encoder I/O: {0}")]
    Io(#[from] std::io::Error),

    #[error("FFmpeg: {0}")]
    Ffmpeg(String),

    #[error("{0}")]
    Msg(String),
}

impl EncodeError {
    pub fn ffmpeg<E: std::fmt::Display>(err: E) -> Self {
        EncodeError::Ffmpeg(err.to_string())
    }

    pub fn msg(s: impl Into<String>) -> Self {
        EncodeError::Msg(s.into())
    }
}

#[cfg(feature = "ffmpeg")]
impl From<ffmpeg_next::Error> for EncodeError {
    fn from(e: ffmpeg_next::Error) -> Self {
        EncodeError::Ffmpeg(e.to_string())
    }
}
