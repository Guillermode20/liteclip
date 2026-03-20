//! # AudioConfig - Trait Implementations
//!
//! This module contains trait implementations for `AudioConfig`.
//!
//! ## Implemented Traits
//!
//! - `Default`
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use super::functions::{
    default_balance,
    default_master_volume, default_mic_device, default_mic_volume, default_system_volume,
    default_true,
};
use super::types::AudioConfig;

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            capture_system: default_true(),
            capture_mic: default_true(),
            mic_device: default_mic_device(),
            mic_volume: default_mic_volume(),
            system_volume: default_system_volume(),
            balance: default_balance(),
            master_volume: default_master_volume(),
            mic_noise_reduction: default_true(),
        }
    }
}
