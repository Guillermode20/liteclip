//! LiteClip Replay - A lightweight Windows screen recording application with replay buffer
//!
//! LiteClip Replay provides low-overhead screen capture with a retroactive save feature.
//! The application continuously records to an in-memory ring buffer, allowing users to
//! save the last N seconds of gameplay or desktop activity on demand.
//!
//! # Architecture
//!
//! The application is organized into several key modules:
//!
//! - [`app`] - Application state management and recording pipeline coordination
//! - [`buffer`] - Lock-free ring buffer for replay storage
//! - [`capture`] - DXGI screen capture and WASAPI audio capture
//! - [`encode`] - Video encoding (NVENC, AMF, QSV, software)
//! - [`clip`] - Clip saving and muxing functionality
//! - [`config`] - Configuration management
//! - [`platform`] - Windows integration (hotkeys, tray, notifications)
//! - [`gui`] - User interface components (settings, Clip & Compress)
//! - [`output`] - Output file handling and thumbnails
//! - [`detection`] - Running game detection
//!
//! # Data Flow
//!
//! ```text
//! ┌─────────────┐     ┌─────────────┐     ┌─────────────┐
//! │   Capture   │────▶│   Encode    │────▶│   Buffer    │
//! │  (DXGI/     │     │  (NVENC/    │     │   (Ring)    │
//! │   WASAPI)   │     │   AMF/SW)   │     │             │
//! └─────────────┘     └─────────────┘     └──────┬──────┘
//!                                                │
//!                       ┌─────────────┐          │
//!                       │   Output    │◀─────────┘
//!                       │   (MP4)     │
//!                       └─────────────┘
//! ```
//!
//! # Feature Flags
//!
//! - `ffmpeg` (default) - Enable FFmpeg-based encoding and muxing
//!
//! # Example
//!
//! ```no_run
//! use liteclip_replay::{app::AppState, config::Config};
//! use std::sync::Arc;
//! use tokio::sync::RwLock;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     // Load configuration
//!     let config = Config::default();
//!     
//!     // Initialize application state
//!     let state = Arc::new(RwLock::new(AppState::new(config)?));
//!     
//!     // Start recording
//!     state.write().await.start_recording().await?;
//!     
//!     Ok(())
//! }
//! ```

#[path = "app/mod.rs"]
pub mod app;
pub mod buffer;
pub mod capture;
pub mod clip;
pub mod config;
pub mod detection;
pub mod encode;
pub mod gui;
pub mod output;
pub mod platform;

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
