//! # HotkeyConfig - Trait Implementations
//!
//! This module contains trait implementations for `HotkeyConfig`.
//!
//! ## Implemented Traits
//!
//! - `Default`
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use super::types::HotkeyConfig;
use super::functions::{default_hotkey_gallery, default_hotkey_save, default_hotkey_screenshot, default_hotkey_toggle};

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            save_clip: default_hotkey_save(),
            toggle_recording: default_hotkey_toggle(),
            screenshot: default_hotkey_screenshot(),
            open_gallery: default_hotkey_gallery(),
        }
    }
}

