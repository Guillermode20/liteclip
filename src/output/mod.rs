//! Output File Handling
//!
//! This module provides functionality for saving encoded packets to MP4 files,
//! generating thumbnails, and managing output paths.
//!
//! # Key Features
//!
//! - MP4 muxing via FFmpeg
//! - Thumbnail extraction from keyframes
//! - Timestamped output file naming
//! - Game-organized folder structure
//!
//! # Key Types
//!
//! - [`Muxer`] - FFmpeg-based MP4 muxer
//! - [`MuxerConfig`] - Muxer configuration
//! - [`OutputError`] - Output-specific errors
//!
//! # Key Functions
//!
//! - [`spawn_clip_saver`] - Spawn a background task to save a clip
//! - [`generate_output_path`] - Generate a timestamped output file path
//! - [`generate_thumbnail`] - Create a thumbnail from encoded video
//! - [`h264_nal_type`] / [`hevc_nal_type`] - Parse NAL unit types
//!
//! # Example
//!
//! ```ignore
//! use liteclip_replay::output::{spawn_clip_saver, MuxerConfig, generate_output_path};
//!
//! // Generate output path
//! let output_path = generate_output_path(&config, Some("game_name"))?;
//!
//! // Configure muxer
//! let muxer_config = MuxerConfig {
//!     video_codec: "hevc",
//!     framerate: 60,
//!     ..Default::default()
//! };
//!
//! // Save clip in background
//! spawn_clip_saver(muxer_config, packets, output_path, None).await?;
//! ```

pub mod error;
pub mod functions;
pub mod mp4;
pub mod saver;
pub mod types;

pub use error::OutputError;
pub use functions::{
    calculate_clip_start_pts, extract_thumbnail, generate_output_path, generate_thumbnail,
    h264_nal_type, hevc_nal_type,
};
pub use saver::{spawn_clip_saver, spawn_clip_saver_with_defaults};
pub use types::{Muxer, MuxerConfig};
