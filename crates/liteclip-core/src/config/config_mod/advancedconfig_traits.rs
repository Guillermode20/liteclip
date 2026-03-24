//! # AdvancedConfig - Trait Implementations
//!
//! This module contains trait implementations for `AdvancedConfig`.
//!
//! ## Implemented Traits
//!
//! - `Default`
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use super::functions::{default_false, default_gpu_index, default_keyframe_interval};
use super::types::AdvancedConfig;

impl Default for AdvancedConfig {
    fn default() -> Self {
        Self {
            memory_limit_mb: 1024,
            gpu_index: default_gpu_index(),
            keyframe_interval_secs: default_keyframe_interval(),
            use_cpu_readback: default_false(),
        }
    }
}
