//! Clip compatibility module.

pub mod muxer;

pub use crate::output::{
    extract_thumbnail, generate_output_path, spawn_clip_saver, spawn_clip_saver_with_defaults,
    Muxer, MuxerConfig,
};
