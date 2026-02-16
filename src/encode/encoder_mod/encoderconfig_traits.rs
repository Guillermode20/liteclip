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
        let use_native_resolution =
            matches!(config.video.resolution, crate::config::Resolution::Native);
        Self {
            codec: config.video.codec,
            bitrate_mbps: config.video.bitrate_mbps,
            framerate: config.video.framerate,
            resolution: match config.video.resolution {
                crate::config::Resolution::Native => (0, 0),
                crate::config::Resolution::P1080 => (1920, 1080),
                crate::config::Resolution::P720 => (1280, 720),
                crate::config::Resolution::P480 => (854, 480),
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
