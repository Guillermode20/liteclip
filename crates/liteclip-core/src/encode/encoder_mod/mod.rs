//! Shared traits and types for all encoder implementations.
//!
//! This module defines the common interfaces used by hardware and software
//! encoders to produce encoded video and audio packets.
//!
//! # Architecture
//!
//! Encoders in LiteClip follow a streaming model:
//! 1. Frames are received from the capture layer.
//! 2. The encoder processes them (using GPU or CPU).
//! 3. Encoded packets are wrapped in `EncodedPacket` and pushed to the buffer.
//!
//! This abstraction allows for swapping backends (e.g., NVENC vs AMF) without
//! changing the rest of the recording pipeline.

pub mod encodedpacket_traits;
pub mod encoderconfig_traits;
mod functions;
mod types;

// Re-export all types
pub use functions::*;
pub use types::*;
