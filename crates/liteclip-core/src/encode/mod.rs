//! Video Encoding
//!
//! This module provides video and audio encoding functionality using FFmpeg.
//! Multiple encoder backends are supported for different hardware configurations.
//!
//! # Supported Encoders
//!
//! | Encoder | Type | Requirements |
//! |--------|------|--------------|
//! | NVENC | Hardware | NVIDIA GPU (GTX 600+) |
//! | AMF | Hardware | AMD GPU |
//! | QSV | Hardware | Intel iGPU |
//! | libx264/libx265 | Software | CPU only |
//!
//! # Architecture
//!
//! The encoding pipeline:
//!
//! 1. Receive captured frames from the capture thread
//! 2. Convert to encoder-compatible format (BGRA → NV12 if needed)
//! 3. Encode video frames via FFmpeg
//! 4. Encode audio packets
//! 5. Output encoded packets to the replay buffer
//!
//! # Key Types
//!
//! - [`Encoder`] - Encoder abstraction trait
//! - [`EncoderFactory`] - Factory trait for spawning encoders
//! - [`DefaultEncoderFactory`] - Default FFmpeg-based factory
//! - [`EncodedPacket`] - Encoded video/audio packet
//! - [`EncodeConfig`] - Encoder configuration
//! - [`EncodeError`] - Encoding-specific errors
//!
//! # Example
//!
//! ```no_run
//! use liteclip_core::encode::EncodedPacket;
//! use liteclip_core::encode::StreamType;
//! use bytes::Bytes;
//!
//! let packet = EncodedPacket::new(
//!     Bytes::from(&b"data"[..]),
//!     0,  // pts
//!     0,  // dts
//!     true,  // is_keyframe
//!     StreamType::Video,
//! );
//! ```

pub mod encoder_mod;
pub mod error;
pub mod ffmpeg;
pub mod sw_encoder;

pub use encoder_mod::*;

pub use error::EncodeError;

pub type EncodeResult<T> = std::result::Result<T, EncodeError>;
