//! Buffer types
//!
//! SharedReplayBuffer wraps LockFreeReplayBuffer for thread-safe access.

use crate::buffer::BufferResult;
use crate::encode::EncodedPacket;

use super::spmc_ring::{LockFreeReplayBuffer, TrackedSnapshot};

/// Thread-safe wrapper around LockFreeReplayBuffer
#[derive(Clone)]
pub struct SharedReplayBuffer {
    inner: LockFreeReplayBuffer,
}

impl SharedReplayBuffer {
    pub fn new(config: &crate::config::Config) -> BufferResult<Self> {
        let inner = LockFreeReplayBuffer::new(config)?;
        Ok(Self { inner })
    }

    pub fn push_batch(&self, packets: impl IntoIterator<Item = EncodedPacket>) {
        self.inner.push_batch(packets);
    }

    pub fn push(&self, packet: EncodedPacket) {
        self.inner.push(packet);
    }

    pub fn snapshot(&self) -> BufferResult<TrackedSnapshot> {
        self.inner.snapshot()
    }

    pub fn snapshot_from(&self, start_pts: i64) -> BufferResult<TrackedSnapshot> {
        self.inner.snapshot_from(start_pts)
    }

    pub fn clear(&self) {
        self.inner.clear();
    }

    /// Completely resets the replay buffer including parameter caches.
    pub fn restart(&self) {
        self.inner.restart();
    }

    pub fn stats(&self) -> BufferStats {
        self.inner.stats()
    }

    pub fn pinned_bytes(&self) -> usize {
        self.inner.pinned_bytes()
    }

    pub fn oldest_pts(&self) -> Option<i64> {
        self.inner.oldest_pts()
    }

    pub fn newest_pts(&self) -> Option<i64> {
        self.inner.newest_pts()
    }

    pub fn snapshot_first_packet_resolution(&self) -> Option<(u32, u32)> {
        self.inner.first_packet_resolution()
    }

    pub fn has_keyframe(&self) -> bool {
        self.inner.has_keyframe()
    }
}

/// Statistics about the buffer state
#[derive(Debug, Clone, Copy, Default)]
pub struct BufferStats {
    /// Current duration in seconds
    pub duration_secs: f64,
    /// Total bytes in buffer
    pub total_bytes: usize,
    /// Number of packets
    pub packet_count: usize,
    /// Keyframe count
    pub keyframe_count: usize,
    /// Memory usage percentage (0-100)
    pub memory_usage_percent: f32,
}
