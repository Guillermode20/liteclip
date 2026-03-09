//! Clip Muxer
//!
//! This module provides MP4 muxing functionality using FFmpeg.
//!
//! The muxer combines encoded video and audio packets into an MP4 container
//! file, handling codec parameters, timestamps, and container metadata.
//!
//! # Usage
//!
//! This module re-exports functionality from the output module for
//! convenience and to provide a clear conceptual boundary between
//! clip management and output handling.
//!
//! # Key Re-exports
//!
//! - `calculate_clip_start_pts` - Calculate clip start timestamp
//! - `generate_output_path` - Generate timestamped output file path
//! - `Muxer` - FFmpeg-based MP4 muxer
//! - `MuxerConfig` - Muxer configuration
//!
//! For full documentation, see the `output` module.

pub use crate::output::functions::*;
pub use crate::output::mp4::*;
pub use crate::output::types::*;
