//! Replay Buffer Implementation
//!
//! This module provides the lock-free ring buffer implementation for storing
//! encoded video and audio packets in memory.
//!
//! # Design
//!
//! SPMC ring with an atomic write index and per-slot mutexes (see `lockfree` module docs).
//!
//! - **Single Producer**: Encoder pushes packets
//! - **Multiple Consumers**: Clip saving reads snapshots
//!
//! # Key Types
//!
//! - [`ReplayBuffer`] - Main buffer handle (type alias for `SharedReplayBuffer`)
//! - [`LockFreeReplayBuffer`] - Core lock-free implementation
//! - [`SharedReplayBuffer`] - Thread-safe wrapper
//! - [`BufferStats`] - Buffer statistics
//!
//! # Memory Model
//!
//! The buffer maintains:
//!
//! - A power-of-two sized ring of packet slots
//! - Atomic write index for the producer
//! - Parameter set cache (SPS/PPS/VPS) for clip saving
//! - Statistics for monitoring (duration, memory usage, keyframes)
//!
//! # Eviction Policy
//!
//! When the buffer is full, old packets are overwritten. Keyframes are
//! tracked to ensure clips always start at a keyframe for proper decoding.
//!
//! # Example
//!
//! ```no_run
//! use liteclip_replay::buffer::ring::ReplayBuffer;
//! use liteclip_replay::config::Config;
//!
//! let config = Config::default();
//! let buffer = ReplayBuffer::new(&config).unwrap();
//!
//! // Check statistics
//! let stats = buffer.stats();
//! println!("Buffer: {:.1}s, {} MB", stats.duration_secs, stats.total_bytes / 1024 / 1024);
//! ```

pub mod functions;
pub mod lockfree;
pub mod types;

pub use functions::*;
pub use lockfree::LockFreeReplayBuffer;
pub use types::{BufferStats, SharedReplayBuffer};

/// Main replay buffer type.
///
/// This is the primary type used throughout the application for
/// storing encoded packets. It wraps `SharedReplayBuffer` for
/// thread-safe access.
pub type ReplayBuffer = SharedReplayBuffer;
