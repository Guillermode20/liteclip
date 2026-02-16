//! Auto-generated module
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use crate::encode::EncodedPacket;
use anyhow::Result;
use parking_lot::RwLock;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, trace};

use super::functions::qpc_frequency;

/// In-memory ring buffer for encoded packets
pub struct ReplayBuffer {
    /// Packet queue (oldest at front)
    packets: VecDeque<EncodedPacket>,
    /// Target duration
    duration: Duration,
    /// Max memory budget in bytes
    max_memory_bytes: usize,
    /// Keyframe index: VecDeque of (pts, relative_index) pairs for O(1) front/back ops
    /// relative_index is the index within the current packets VecDeque
    keyframe_index: VecDeque<(i64, usize)>,
    /// Total bytes currently stored
    total_bytes: usize,
}
impl ReplayBuffer {
    /// Create new replay buffer from configuration
    pub fn new(config: &crate::config::Config) -> Result<Self> {
        let duration = Duration::from_secs(config.general.replay_duration_secs as u64);
        let max_memory_bytes =
            (config.advanced.memory_limit_mb as usize).saturating_mul(1024 * 1024);
        debug!(
            "Creating ReplayBuffer: {} seconds, {} MB max",
            duration.as_secs(),
            config.advanced.memory_limit_mb
        );
        Ok(Self {
            packets: VecDeque::new(),
            duration,
            max_memory_bytes,
            keyframe_index: VecDeque::new(),
            total_bytes: 0,
        })
    }
    /// Create a new replay buffer with explicit parameters
    pub fn with_params(duration: Duration, max_memory_mb: usize) -> Self {
        let max_memory_bytes = max_memory_mb.saturating_mul(1024 * 1024);
        debug!(
            "Creating ReplayBuffer: {} seconds, {} MB max",
            duration.as_secs(),
            max_memory_mb
        );
        Self {
            packets: VecDeque::new(),
            duration,
            max_memory_bytes,
            keyframe_index: VecDeque::new(),
            total_bytes: 0,
        }
    }
    /// Push a new packet into the buffer (evicts old if needed based on duration)
    ///
    /// Uses atomic eviction - calculates and executes eviction in a single loop
    /// to avoid TOCTOU race conditions where packets could be added between
    /// calculation and eviction.
    pub fn push(&mut self, packet: EncodedPacket) {
        let packet_size = packet.data.len();
        let target_duration_qpc = (self.duration.as_secs_f64() * qpc_frequency() as f64) as i64;

        // Atomic eviction: check condition and evict in the same loop
        // This prevents race conditions where new packets could be added
        // between calculation and eviction.
        while !self.packets.is_empty() {
            let oldest_pts = self.packets.front().map(|p| p.pts).unwrap_or(packet.pts);
            let projected_span = packet.pts.saturating_sub(oldest_pts);
            if projected_span <= target_duration_qpc {
                break;
            }
            self.evict_oldest();
        }

        // Memory limit eviction - also atomic
        while self.total_bytes + packet_size > self.max_memory_bytes && !self.packets.is_empty() {
            self.evict_oldest();
        }

        // Now safe to add the new packet
        if packet.is_keyframe {
            let relative_index = self.packets.len();
            self.keyframe_index.push_back((packet.pts, relative_index));
        }
        self.total_bytes += packet_size;
        self.packets.push_back(packet);

        if self.packets.len() % 100 == 0 {
            trace!(
                "Buffer: {} packets, {} bytes, {} keyframes",
                self.packets.len(),
                self.total_bytes,
                self.keyframe_index.len()
            );
        }
    }
    /// Evict the oldest packet — O(1) operation
    fn evict_oldest(&mut self) {
        if let Some(packet) = self.packets.pop_front() {
            self.total_bytes -= packet.data.len();
            // Remove keyframe index entry if this was a keyframe
            // The keyframe index stores relative indices, so we just pop_front
            // if the oldest keyframe index refers to position 0
            if packet.is_keyframe {
                // Remove entries with relative_index == 0 (the evicted packet)
                self.keyframe_index.retain(|&(_, rel_idx)| rel_idx > 0);
            }
            // Decrement all relative indices by 1
            for (_, rel_idx) in self.keyframe_index.iter_mut() {
                *rel_idx -= 1;
            }
        }
    }
    /// Get a snapshot of all packets (cheap clone via Bytes)
    pub fn snapshot(&self) -> Result<Vec<EncodedPacket>> {
        let mut result = Vec::with_capacity(self.packets.len());
        result.extend(self.packets.iter().cloned());
        Ok(result)
    }
    /// Get packets from timestamp to now
    ///
    /// Finds the nearest keyframe at or before start_pts and returns all packets
    /// from that point forward. This ensures the video can be decoded properly.
    pub fn snapshot_from(&self, start_pts: i64) -> Result<Vec<EncodedPacket>> {
        // Find the last keyframe at or before start_pts using binary search
        // keyframe_index is sorted by pts since packets are added in chronological order
        let start_index = self.find_keyframe_index_before(start_pts).unwrap_or(0);
        let remaining = self.packets.len().saturating_sub(start_index);
        let mut result = Vec::with_capacity(remaining);
        result.extend(self.packets.iter().skip(start_index).cloned());
        Ok(result)
    }

    /// Find the relative index of the last keyframe at or before the given pts.
    /// Uses binary search for O(log N) performance.
    fn find_keyframe_index_before(&self, target_pts: i64) -> Option<usize> {
        // Since keyframe_index is a VecDeque and pts values are monotonically increasing,
        // we can binary search on the underlying slices.
        // VecDeque::as_slices() returns (&[T], &[T]) - the logical contiguous slice
        // may be split across the ring buffer boundary.
        let (front, back) = self.keyframe_index.as_slices();

        // Try searching the back slice first (usually contains newer keyframes)
        if let Ok(idx) = back.binary_search_by(|&(pts, _)| pts.cmp(&target_pts)) {
            // Exact match found in back slice
            return Some(back[idx].1);
        } else if let Ok(idx) = front.binary_search_by(|&(pts, _)| pts.cmp(&target_pts)) {
            // Exact match found in front slice
            return Some(front[idx].1);
        }

        // No exact match - find the closest keyframe before target_pts
        // Search from the end backwards for efficiency (common case)
        for &(pts, rel_idx) in self.keyframe_index.iter().rev() {
            if pts <= target_pts {
                return Some(rel_idx);
            }
        }

        None
    }
    /// Get the last N seconds of packets based on duration
    pub fn snapshot_last(&self, duration: Duration) -> Result<Vec<EncodedPacket>> {
        if self.packets.is_empty() {
            return Ok(vec![]);
        }
        let newest_pts = self.packets.back().map(|p| p.pts).unwrap_or(0);
        let qpc_freq = qpc_frequency() as f64;
        let qpc_delta = (duration.as_secs_f64() * qpc_freq) as i64;
        let start_pts = (newest_pts - qpc_delta).max(0);
        self.snapshot_from(start_pts)
    }
    /// Clear all packets
    pub fn clear(&mut self) {
        self.packets.clear();
        self.keyframe_index.clear();
        self.total_bytes = 0;
        debug!("Buffer cleared");
    }
    /// Get current buffer statistics
    pub fn stats(&self) -> BufferStats {
        let keyframe_count = self.keyframe_index.len();
        let memory_usage_percent = if self.max_memory_bytes > 0 {
            (self.total_bytes as f32 / self.max_memory_bytes as f32) * 100.0
        } else {
            0.0
        };
        let qpc_freq_f64 = qpc_frequency() as f64;
        let duration_secs = if self.packets.len() >= 2 {
            let first = self.packets.front().map(|p| p.pts).unwrap_or(0);
            let last = self.packets.back().map(|p| p.pts).unwrap_or(0);
            ((last - first) as f64) / qpc_freq_f64
        } else {
            0.0
        };
        BufferStats {
            duration_secs,
            total_bytes: self.total_bytes,
            packet_count: self.packets.len(),
            keyframe_count,
            memory_usage_percent: memory_usage_percent.min(100.0),
        }
    }
    /// Check if buffer is at duration limit
    pub fn is_full(&self) -> bool {
        if self.packets.len() < 2 {
            return false;
        }
        let target_duration_qpc = (self.duration.as_secs_f64() * qpc_frequency() as f64) as i64;
        let first = self.packets.front().map(|p| p.pts).unwrap_or(0);
        let last = self.packets.back().map(|p| p.pts).unwrap_or(0);
        last.saturating_sub(first) >= target_duration_qpc
    }
    /// Get the oldest packet timestamp
    pub fn oldest_pts(&self) -> Option<i64> {
        self.packets.front().map(|p| p.pts)
    }
    /// Get the newest packet timestamp
    pub fn newest_pts(&self) -> Option<i64> {
        self.packets.back().map(|p| p.pts)
    }
    /// Get configured max duration
    pub fn duration(&self) -> Duration {
        self.duration
    }
    /// Get max memory budget in bytes
    pub fn max_memory_bytes(&self) -> usize {
        self.max_memory_bytes
    }
    /// Get number of keyframes in index
    pub fn keyframe_count(&self) -> usize {
        self.keyframe_index.len()
    }
    /// Get the resolution from the first packet in the buffer
    pub fn first_packet_resolution(&self) -> Option<(u32, u32)> {
        self.packets.front().and_then(|p| p.resolution)
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
/// Thread-safe wrapper around ReplayBuffer
pub struct SharedReplayBuffer {
    pub(super) inner: Arc<RwLock<ReplayBuffer>>,
}
impl SharedReplayBuffer {
    /// Create a new shared replay buffer
    pub fn new(config: &crate::config::Config) -> Result<Self> {
        let inner = ReplayBuffer::new(config)?;
        Ok(Self {
            inner: Arc::new(RwLock::new(inner)),
        })
    }
    /// Push a packet (acquires write lock)
    pub fn push(&self, packet: EncodedPacket) {
        self.inner.write().push(packet);
    }
    /// Get a snapshot of all packets (acquires read lock)
    pub fn snapshot(&self) -> Result<Vec<EncodedPacket>> {
        self.inner.read().snapshot()
    }
    /// Get packets from a specific timestamp
    pub fn snapshot_from(&self, start_pts: i64) -> Result<Vec<EncodedPacket>> {
        self.inner.read().snapshot_from(start_pts)
    }
    /// Clear all packets
    pub fn clear(&self) {
        self.inner.write().clear();
    }
    /// Get current statistics
    pub fn stats(&self) -> BufferStats {
        self.inner.read().stats()
    }
    /// Check if buffer is full
    pub fn is_full(&self) -> bool {
        self.inner.read().is_full()
    }
    /// Get the oldest packet timestamp
    pub fn oldest_pts(&self) -> Option<i64> {
        self.inner.read().oldest_pts()
    }
    /// Get the newest packet timestamp
    pub fn newest_pts(&self) -> Option<i64> {
        self.inner.read().newest_pts()
    }
    /// Get the resolution from the first packet in the buffer
    pub fn snapshot_first_packet_resolution(&self) -> Option<(u32, u32)> {
        self.inner.read().first_packet_resolution()
    }
}
