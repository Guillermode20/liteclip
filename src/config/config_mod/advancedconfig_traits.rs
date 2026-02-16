//! # AdvancedConfig - Trait Implementations
//!
//! This module contains trait implementations for `AdvancedConfig`.
//!
//! ## Implemented Traits
//!
//! - `Default`
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use super::functions::{
    default_gpu_index, default_keyframe_interval, default_memory_limit, default_overlay_position,
    default_true,
};
use super::types::AdvancedConfig;

impl Default for AdvancedConfig {
    fn default() -> Self {
        Self {
            memory_limit_mb: default_memory_limit(),
            gpu_index: default_gpu_index(),
            keyframe_interval_secs: default_keyframe_interval(),
            overlay_enabled: default_true(),
            overlay_position: default_overlay_position(),
            use_cpu_readback: default_true(),
        }
    }
}
