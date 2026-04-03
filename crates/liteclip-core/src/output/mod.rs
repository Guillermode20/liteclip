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
//! ```no_run
//! use liteclip_core::output::{MuxerConfig, generate_output_path};
//! use std::path::Path;
//!
//! // Generate output path
//! let output_path = generate_output_path(Path::new("C:/Videos"), Some("game_name")).unwrap();
//!
//! // Configure muxer
//! let muxer_config = MuxerConfig::new(1920, 1080, 60.0, output_path);
//! ```

pub mod companion_cache;
pub mod error;
pub mod functions;
#[cfg(feature = "ffmpeg")]
pub mod mp4;
pub mod saver;
#[cfg(feature = "ffmpeg")]
pub mod sdk_export;
#[cfg(feature = "ffmpeg")]
pub mod sdk_ffmpeg_output;
pub mod types;
pub mod video_file;

pub use companion_cache::hash_main_video_path;
pub use error::{OutputError, OutputResult};
pub use functions::{
    calculate_clip_start_pts, ffmpeg_executable_path, generate_output_path, generate_thumbnail,
    h264_nal_type, hevc_nal_type,
};
pub use saver::{spawn_clip_saver, SKIP_THUMBNAIL_ENV};
pub use types::{Muxer, MuxerConfig};
pub use video_file::{
    estimate_export_bitrates, extract_preview_frame, probe_video_file, spawn_clip_export,
    ClipExportPhase, ClipExportRequest, ClipExportUpdate, ExportBitrateEstimate, TimeRange,
    VideoFileMetadata,
};
