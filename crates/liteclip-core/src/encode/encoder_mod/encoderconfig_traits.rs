//! # EncoderConfig - Trait Implementations
//!
//! This module contains trait implementations for `EncoderConfig`.
//!
//! ## Implemented Traits
//!
//! - `From`
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use super::types::EncoderConfig;

impl From<&crate::config::Config> for EncoderConfig {
    fn from(config: &crate::config::Config) -> Self {
        let use_native_resolution = config.video.use_native_resolution
            || matches!(config.video.resolution, crate::config::Resolution::Native);
        Self {
            bitrate_mbps: config.video.bitrate_mbps,
            framerate: config.video.framerate,
            resolution: match config.video.resolution {
                crate::config::Resolution::Native => (0, 0),
                crate::config::Resolution::P480 => (854, 480),
                crate::config::Resolution::P720 => (1280, 720),
                crate::config::Resolution::P1080 => (1920, 1080),
                crate::config::Resolution::P1440 => (2560, 1440),
                crate::config::Resolution::P2160 => (3840, 2160),
                crate::config::Resolution::UW1080 => (2560, 1080),
                crate::config::Resolution::UW1440 => (3440, 1440),
                crate::config::Resolution::UW2160 => (5120, 2160),
                crate::config::Resolution::SuperUW => (3840, 1080),
                crate::config::Resolution::SuperUW1440 => (5120, 1440),
                crate::config::Resolution::Custom(width, height) => (width, height),
            },
            use_native_resolution,
            encoder_type: config.video.encoder,
            quality_preset: config.video.quality_preset,
            rate_control: config.video.rate_control,
            quality_value: config.video.quality_value,
            keyframe_interval_secs: config.advanced.keyframe_interval_secs,
            use_cpu_readback: config.advanced.use_cpu_readback,
            output_index: config.advanced.gpu_index,
        }
    }
}
