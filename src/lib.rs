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
//! - [`buffer`] - SPMC ring buffer for replay storage
//! - [`capture`] - DXGI screen capture and WASAPI audio capture
//! - [`encode`] - Video encoding (NVENC, AMF, QSV, software)
//! - [`config`] - Configuration management
//! - [`platform`] - Windows integration (hotkeys, tray, notifications)
//! - [`gui`] - User interface components (settings, Clip & Compress)
//! - [`output`] - Output file handling, muxing, and thumbnails
//! - [`detection`] - Running game detection
//! - [`media`] - Shared frame types used by capture and encode
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
//! use std::sync::{Arc, Mutex};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let config = Config::default();
//!     let state = Arc::new(Mutex::new(AppState::new(config)?));
//!     state.lock().unwrap().start_recording()?;
//!     Ok(())
//! }
//! ```

pub mod app;
pub mod buffer;
pub mod capture;
pub mod config;
pub mod detection;
pub mod encode;
pub mod gui;
pub mod hotkey_parse;
pub mod media;
pub mod output;
pub mod platform;
