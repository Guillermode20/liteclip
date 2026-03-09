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
//! - [`gui`] - User interface components (settings, gallery)
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
pub mod core;
pub mod d3d;
pub mod detection;
pub mod encode;
pub mod gui;
pub mod metrics;
pub mod output;
pub mod platform;

use std::sync::Arc;
use tokio::sync::RwLock;

/// Application state handle shared across threads.
///
/// This type alias provides a thread-safe reference to the application state
/// that can be cloned and shared between different parts of the application.
///
/// # Thread Safety
///
/// Uses `Arc<RwLock<...>>` to allow multiple readers or a single writer.
/// The `RwLock` is async-aware (from tokio) for non-blocking concurrent access.
pub type AppHandle = Arc<RwLock<app::AppState>>;

/// Result type alias using anyhow for ergonomic error handling.
///
/// Use this for functions that can fail with any error type that implements
/// `std::error::Error + Send + Sync + 'static`.
pub type Result<T> = anyhow::Result<T>;

/// Error type alias for anyhow errors.
pub type Error = anyhow::Error;

/// Initialize tracing subscriber for structured logging.
///
/// Sets up a default tracing subscriber that outputs to stdout with
/// reasonable defaults for application logging.
///
/// # Example
///
/// ```ignore
/// liteclip_replay::init_logging();
/// tracing::info!("Application started");
/// ```
pub fn init_logging() {
    tracing_subscriber::fmt::init();
}

/// Converts a [`config::Config`] reference into [`platform::HotkeyConfig`].
///
/// This implementation extracts hotkey configuration from the main config
/// for use by the platform layer.
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
