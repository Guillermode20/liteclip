//! # GeneralConfig - Trait Implementations
//!
//! This module contains trait implementations for `GeneralConfig`.
//!
//! ## Implemented Traits
//!
//! - `Default`
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use super::functions::{default_replay_duration, default_save_directory, default_true};
use super::types::GeneralConfig;

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            replay_duration_secs: default_replay_duration(),
            save_directory: default_save_directory(),
            auto_start_with_windows: default_true(),
            start_minimised: default_true(),
            notifications: default_true(),
            auto_detect_game: default_true(),
            generate_clip_thumbnail: default_true(),
        }
    }
}
