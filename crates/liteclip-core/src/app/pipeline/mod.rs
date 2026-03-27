//! Recording Pipeline
//!
//! This module orchestrates the capture → encode → buffer data flow.
//!
//! # Architecture
//!
//! The recording pipeline manages the following components:
//!
//! 1. **Video Capture**: DXGI Desktop Duplication thread
//! 2. **Audio Capture**: WASAPI loopback and microphone threads
//! 3. **Video Encoding**: FFmpeg encoder thread
//! 4. **Audio Encoding**: AAC encoder thread
//! 5. **Buffer**: Lock-free ring buffer for packet storage
//!
//! # Lifecycle
//!
//! The pipeline has three states:
//!
//! - **Stopped**: No capture or encoding threads running
//! - **Starting**: Threads are being spawned
//! - **Running**: Active capture and encoding
//!
//! # Key Types
//!
//! - [`RecordingPipeline`] - Main pipeline manager
//! - [`RecordingLifecycle`] - Lifecycle state enum
//! - [`AudioForwardHandle`] - Handle for audio forwarding thread lifecycle
//! - [`AudioCaptureResult`] - Result from starting audio capture
//!
//! # Example
//!
//! ```no_run
//! use liteclip_core::app::pipeline::RecordingPipeline;
//! use liteclip_core::buffer::ReplayBuffer;
//! use liteclip_core::config::Config;
//!
//! let mut pipeline = RecordingPipeline::with_defaults();
//! let config = Config::default();
//! let buffer = ReplayBuffer::new(&config).unwrap();
//!
//! // Start recording
//! // pipeline.start(&config, &buffer).unwrap();
//! ```

pub mod audio;
pub mod lifecycle;
pub mod manager;
pub mod video;

pub use audio::{AudioCaptureResult, AudioForwardHandle};
pub use lifecycle::RecordingLifecycle;
pub use manager::RecordingPipeline;
