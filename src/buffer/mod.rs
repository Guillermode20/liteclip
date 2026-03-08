//! Ring Buffer Management
//!
//! Maintains a rolling window of encoded packets in memory.
//! Uses lock-free ring buffer for optimal producer/consumer performance.

pub mod error;
pub mod ring;

pub use error::BufferError;
pub use ring::{BufferStats, ReplayBuffer, SharedReplayBuffer};
