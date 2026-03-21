//! # VideoConfig - Trait Implementations
//!
//! This module contains trait implementations for `VideoConfig`.
//!
//! ## Implemented Traits
//!
//! - `Default`
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use super::functions::{
    default_bitrate, default_encoder, default_false, default_framerate, default_quality_preset,
    default_quality_value, default_rate_control, default_resolution, default_true,
    default_webcam_height, default_webcam_width,
};
use super::types::VideoConfig;

impl Default for VideoConfig {
    fn default() -> Self {
        Self {
            resolution: default_resolution(),
            framerate: default_framerate(),
            bitrate_mbps: default_bitrate(),
            encoder: default_encoder(),
            quality_preset: default_quality_preset(),
            rate_control: default_rate_control(),
            quality_value: default_quality_value(),
            use_native_resolution: default_true(),
            webcam_enabled: default_false(),
            webcam_device_name: String::new(),
            webcam_width: default_webcam_width(),
            webcam_height: default_webcam_height(),
        }
    }
}
