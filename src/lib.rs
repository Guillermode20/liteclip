//! LiteClip Replay - A lightweight Windows screen recording application with replay buffer
//!
//! Phase 1 Architecture:
//! - DXGI Desktop Duplication for capture
//! - FFmpeg C API for encoding
//! - In-memory ring buffer with Bytes crate
//! - Hidden HWND thread for hotkeys
//! - CLI-only interface (GUI in Phase 2)

pub mod app;
pub mod buffer;
pub mod capture;
pub mod clip;
pub mod config;
pub mod d3d;
pub mod encode;
pub mod gui;
pub mod platform;

use std::sync::Arc;
use tokio::sync::RwLock;

/// Application state handle shared across threads
pub type AppHandle = Arc<RwLock<app::AppState>>;

/// Result type alias using anyhow
pub type Result<T> = anyhow::Result<T>;

/// Error type alias
pub type Error = anyhow::Error;

/// Initialize tracing subscriber for structured logging
pub fn init_logging() {
    tracing_subscriber::fmt::init();
}

/// Hotkey configuration conversion
impl From<&config::Config> for platform::HotkeyConfig {
    fn from(config: &config::Config) -> Self {
        Self {
            save_clip: config.hotkeys.save_clip.clone(),
            toggle_recording: config.hotkeys.toggle_recording.clone(),
            screenshot: config.hotkeys.screenshot.clone(),
            open_gallery: config.hotkeys.open_gallery.clone(),
        }
    }
}
