//! Ring Buffer Management
//!
//! Maintains a rolling window of encoded packets in memory.
//! Uses Bytes crate for reference-counted data and parking_lot::RwLock for thread safety.

pub mod ring;

pub use ring::{BufferStats, ReplayBuffer, SharedReplayBuffer};
