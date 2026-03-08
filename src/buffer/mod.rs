//! Ring Buffer Management
//!
//! Maintains a rolling window of encoded packets in memory.
//! Uses lock-free ring buffer for optimal producer/consumer performance.

pub mod ring;

pub use ring::{BufferStats, SharedReplayBuffer};
