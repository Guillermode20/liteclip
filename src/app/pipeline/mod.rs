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
//!
//! # Example
//!
//! ```ignore
//! use liteclip_replay::app::pipeline::RecordingPipeline;
//! use liteclip_replay::buffer::ReplayBuffer;
//! use liteclip_replay::config::Config;
//!
//! let mut pipeline = RecordingPipeline::new();
//! let buffer = ReplayBuffer::new(&config)?;
//!
//! // Start recording
//! pipeline.start(&config, &buffer).await?;
//!
//! // Check lifecycle
//! assert!(matches!(pipeline.lifecycle(), RecordingLifecycle::Running));
//!
//! // Stop recording
//! pipeline.stop().await?;
//! ```

pub mod audio;
pub mod lifecycle;
pub mod manager;
pub mod video;

pub use lifecycle::RecordingLifecycle;
pub use manager::RecordingPipeline;
