//! Clip Management and Muxing
//!
//! This module provides functionality for saving clips from the replay buffer
//! to disk in MP4 format.
//!
//! # Architecture
//!
//! The clip saving process involves:
//!
//! 1. Snapshotting the replay buffer to get all encoded packets
//! 2. Prepending codec parameter sets (SPS/PPS/VPS) if needed
//! 3. Muxing video and audio streams into MP4 container via FFmpeg
//! 4. Generating a thumbnail for the gallery
//!
//! # Key Types
//!
//! - [`Muxer`] - FFmpeg-based MP4 muxer
//! - [`MuxerConfig`] - Configuration for muxing (codecs, framerate, etc.)
//!
//! # Functions
//!
//! - [`spawn_clip_saver`] - Spawn a background task to save a clip
//! - [`generate_output_path`] - Generate a timestamped output file path
//! - [`generate_thumbnail`] - Extract a thumbnail from encoded video
//!
//! # Example
//!
//! ```no_run
//! use liteclip_replay::output::MuxerConfig;
//! use std::path::PathBuf;
//!
//! let config = MuxerConfig::new(1920, 1080, 60.0, PathBuf::from("output.mp4"));
//! ```

pub use crate::output::{
    generate_output_path, generate_thumbnail, spawn_clip_saver, Muxer, MuxerConfig,
};
