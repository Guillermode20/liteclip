//! Ring Buffer Management
//!
//! This module provides the replay buffer implementation for storing encoded
//! video and audio packets in memory.
//!
//! # Architecture
//!
//! SPMC ring: atomic write index plus mutex-backed slots (see `ring::lockfree`).
//!
//! - **Producer**: Encoding pipeline pushes packets
//! - **Consumer**: Clip saver snapshots the ring; locking model documented in [`ring::lockfree`](crate::buffer::ring::lockfree)
//!
//! # Key Types
//!
//! - [`ReplayBuffer`] - Main buffer handle with configuration-based capacity
//! - [`SharedReplayBuffer`] - Handle wrapping the ring implementation
//! - [`BufferStats`] - Statistics about buffer utilization
//!
//! # Memory Management
//!
//! The buffer enforces both duration and memory limits:
//!
//! - Duration: Configured via `replay_duration_secs`
//! - Memory: Configured via `replay_memory_limit_mb`
//!
//! When either limit is exceeded, old packets are evicted (oldest first,
//! but prefer evicting non-keyframes to maintain seek capability).
//!
//! # Example
//!
//! ```no_run
//! use liteclip_replay::buffer::ReplayBuffer;
//! use liteclip_replay::config::Config;
//!
//! let config = Config::default();
//! let buffer = ReplayBuffer::new(&config).unwrap();
//!
//! // Check statistics
//! let stats = buffer.stats();
//! println!("Buffer: {:.1}s, {} MB", stats.duration_secs, stats.total_bytes / 1024 / 1024);
//! ```

pub mod error;
pub mod ring;

pub use error::{BufferError, BufferResult};
pub use ring::{BufferStats, ReplayBuffer, SharedReplayBuffer};
