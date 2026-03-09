//! Application State and Recording Pipeline
//!
//! This module provides the core application state management and recording
//! pipeline coordination for LiteClip Replay.
//!
//! # Architecture
//!
//! The application layer consists of three main components:
//!
//! - [`AppState`] - Central state coordinator managing configuration, buffer, and pipeline
//! - [`RecordingPipeline`] - Orchestrates the capture → encode → buffer flow
//! - [`ClipManager`] - Handles saving clips from the replay buffer to disk
//!
//! # Example
//!
//! ```ignore
//! use liteclip_replay::app::AppState;
//! use liteclip_replay::config::Config;
//!
//! let config = Config::default();
//! let mut state = AppState::new(config)?;
//!
//! // Start recording
//! state.start_recording().await?;
//!
//! // Save a clip
//! let path = state.save_clip(None).await?;
//! ```

pub mod clip;
pub mod pipeline;
pub mod state;

pub use clip::ClipManager;
pub use pipeline::{RecordingLifecycle, RecordingPipeline};
pub use state::AppState;
