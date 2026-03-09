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
//! - [`EncodedPacket`] - Encoded video/audio packet
//! - [`EncodeConfig`] - Encoder configuration
//! - [`EncodeError`] - Encoding-specific errors
//!
//! # Example
//!
//! ```ignore
//! use liteclip_replay::encode::{Encoder, EncodeConfig};
//!
//! let config = EncodeConfig {
//!     encoder: EncoderType::Nvenc,
//!     codec: VideoCodec::Hevc,
//!     bitrate_mbps: 10,
//!     framerate: 60,
//!     ..Default::default()
//! };
//!
//! let encoder = Encoder::new(config)?;
//! encoder.start()?;
//!
//! // Send frames for encoding
//! encoder.send_frame(frame)?;
//!
//! // Receive encoded packets
//! while let Ok(packet) = encoder.packet_rx().recv() {
//!     buffer.push(packet);
//! }
//! ```

pub mod config;
pub mod encoder_mod;
pub mod ffmpeg;
pub mod packet;
pub mod spawn;
pub mod sw_encoder;

pub use encoder_mod::*;
pub use sw_encoder::*;
