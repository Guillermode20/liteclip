//! LiteClip core — Windows screen capture, encoding, replay ring buffer, and muxing.
//!
//! This crate is the engine behind LiteClip Replay. Host applications (tray apps, games,
//! overlays) can depend on it to run continuous capture into a retroactive buffer and
//! save clips on demand.
//!
//! # Modules
//!
//! - [`app`] — [`AppState`](app::AppState), recording pipeline, clip save orchestration
//! - [`buffer`] — SPMC ring buffer for replay storage
//! - [`capture`] — DXGI video and WASAPI audio capture
//! - [`encode`] — Video encoding (NVENC, AMF, QSV, software) via FFmpeg
//! - [`config`] — Configuration types and persistence
//! - [`output`] — Muxing, thumbnails, clip export helpers
//! - [`media`] — Shared frame types for capture and encode
//! - [`hotkey_parse`] — Hotkey string parsing (shared with hosts that register hotkeys)
//!
//! # Data flow
//!
//! ```text
//! Capture → Encode → Buffer → Output (on save)
//! ```
//!
//! # Feature flags
//!
//! - `ffmpeg` (default) — FFmpeg-based encoding and muxing
//!
//! # Example
//!
//! ```no_run
//! use liteclip_core::{app::AppState, config::Config, encode};
//! use std::sync::{Arc, Mutex};
//!
//! fn main() -> anyhow::Result<()> {
//!     encode::init_ffmpeg()?;
//!     let config = Config::default();
//!     let mut state = AppState::new(config)?;
//!     state.start_recording()?;
//!     Ok(())
//! }
//! ```

pub mod app;
pub mod buffer;
pub mod capture;
pub mod config;
pub mod encode;
pub mod hotkey_parse;
pub mod media;
pub mod output;
