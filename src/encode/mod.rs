//! Video encoding module
//!
//! Encodes captured frames to H.264/H.265/AV1 using native FFmpeg APIs.
//! Supports hardware encoders (NVENC/AMF/QSV) and software encoding.

pub mod encoder_mod;
pub mod ffmpeg_encoder;
pub mod sw_encoder;

pub use encoder_mod::*;
pub use ffmpeg_encoder::*;
pub use sw_encoder::*;
