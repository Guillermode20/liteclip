//! # VideoConfig - Trait Implementations
//!
//! This module contains trait implementations for `VideoConfig`.
//!
//! ## Implemented Traits
//!
//! - `Default`
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use super::types::VideoConfig;
use super::functions::{default_bitrate, default_codec, default_encoder, default_framerate, default_quality_preset, default_quality_value, default_rate_control, default_resolution};

impl Default for VideoConfig {
    fn default() -> Self {
        Self {
            resolution: default_resolution(),
            framerate: default_framerate(),
            codec: default_codec(),
            bitrate_mbps: default_bitrate(),
            encoder: default_encoder(),
            quality_preset: default_quality_preset(),
            rate_control: default_rate_control(),
            quality_value: default_quality_value(),
        }
    }
}

