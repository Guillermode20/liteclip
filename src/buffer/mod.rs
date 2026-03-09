//! Ring Buffer Management
//!
//! This module provides the replay buffer implementation for storing encoded
//! video and audio packets in memory.
//!
//! # Architecture
//!
//! The replay buffer uses a lock-free ring buffer design optimized for the
//! single-producer, multi-consumer (SPMC) pattern:
//!
//! - **Producer**: Encoding pipeline pushes packets atomically
//! - **Consumer**: Clip saver reads snapshots without blocking the producer
//!
//! # Key Types
//!
//! - [`ReplayBuffer`] - Main buffer handle with configuration-based capacity
//! - [`SharedReplayBuffer`] - Thread-safe wrapper around the lock-free core
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
//! ```ignore
//! use liteclip_replay::buffer::ReplayBuffer;
//! use liteclip_replay::config::Config;
//!
//! let config = Config::default();
//! let buffer = ReplayBuffer::new(&config)?;
//!
//! // Push encoded packets
//! buffer.push(packet);
//!
//! // Get a snapshot for saving
//! let packets = buffer.snapshot()?;
//!
//! // Check statistics
//! let stats = buffer.stats();
//! println!("Duration: {:.1}s", stats.duration_secs);
//! ```

pub mod ring;

pub use ring::{BufferStats, ReplayBuffer, SharedReplayBuffer};
