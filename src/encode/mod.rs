//! Video encoding module
//!
//! Encodes captured frames to H.264/H.265/AV1 using native FFmpeg APIs.
//! Supports hardware encoders (NVENC/AMF/QSV) and software encoding.

pub mod config;
pub mod encoder_mod;
pub mod error;
pub mod ffmpeg;
mod ffmpeg_encoder;
pub mod packet;
pub mod spawn;
pub mod sw_encoder;

#[allow(unused_imports)]
pub use config::*;
pub use encoder_mod::*;
pub use error::EncodeError;
#[allow(unused_imports)]
pub use ffmpeg::*;
#[allow(unused_imports)]
pub use packet::*;
#[allow(unused_imports)]
pub use spawn::*;
pub use sw_encoder::*;
