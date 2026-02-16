//! Auto-generated module
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use crate::encode::EncodedPacket;
use anyhow::Result;
use parking_lot::RwLock;
use std::collections::{BTreeMap, VecDeque};
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
    /// Keyframe index: QPC timestamp -> absolute packet index
    keyframe_index: BTreeMap<i64, usize>,
    /// Total bytes currently stored
    total_bytes: usize,
    /// Number of packets evicted from the front (used to adjust keyframe indices)
    base_offset: usize,
}
impl ReplayBuffer {
    /// Create new replay buffer from configuration
    pub fn new(config: &crate::config::Config) -> Result<Self> {
        let duration = Duration::from_secs(config.general.replay_duration_secs as u64);
        let max_memory_bytes = (config.advanced.memory_limit_mb as usize)
            .saturating_mul(1024 * 1024);
        debug!(
            "Creating ReplayBuffer: {} seconds, {} MB max", duration.as_secs(), config
            .advanced.memory_limit_mb
        );
        Ok(Self {
            packets: VecDeque::new(),
            duration,
            max_memory_bytes,
            keyframe_index: BTreeMap::new(),
            total_bytes: 0,
            base_offset: 0,
        })
    }
    /// Create a new replay buffer with explicit parameters
    pub fn with_params(duration: Duration, max_memory_mb: usize) -> Self {
        let max_memory_bytes = max_memory_mb.saturating_mul(1024 * 1024);
        debug!(
            "Creating ReplayBuffer: {} seconds, {} MB max", duration.as_secs(),
            max_memory_mb
        );
        Self {
            packets: VecDeque::new(),
            duration,
            max_memory_bytes,
            keyframe_index: BTreeMap::new(),
            total_bytes: 0,
            base_offset: 0,
        }
    }
    /// Push a new packet into the buffer (evicts old if needed based on duration)
    pub fn push(&mut self, packet: EncodedPacket) {
        let packet_size = packet.data.len();
        let target_duration_qpc = (self.duration.as_secs_f64() * qpc_frequency() as f64)
            as i64;
        let mut packets_to_evict = 0;
        while !self.packets.is_empty() {
            let oldest_pts = self.packets.front().map(|p| p.pts).unwrap_or(packet.pts);
            let projected_span = packet.pts.saturating_sub(oldest_pts);
            if projected_span <= target_duration_qpc {
                break;
            }
            packets_to_evict += 1;
            if packets_to_evict >= 10 {
                break;
            }
        }
        for _ in 0..packets_to_evict {
            if self.packets.is_empty() {
                break;
            }
            self.evict_oldest();
        }
        let mut memory_evictions = 0;
        while self.total_bytes + packet_size > self.max_memory_bytes
            && !self.packets.is_empty()
        {
            self.evict_oldest();
            memory_evictions += 1;
            if memory_evictions >= 10 {
                break;
            }
        }
        if packet.is_keyframe {
            let abs_index = self.base_offset + self.packets.len();
            self.keyframe_index.insert(packet.pts, abs_index);
        }
        self.total_bytes += packet_size;
        self.packets.push_back(packet);
        if self.packets.len() % 100 == 0 {
            trace!(
                "Buffer: {} packets, {} bytes, {} keyframes", self.packets.len(), self
                .total_bytes, self.keyframe_index.len()
            );
        }
    }
    /// Evict the oldest packet — O(1) operation
    fn evict_oldest(&mut self) {
        if let Some(packet) = self.packets.pop_front() {
            self.total_bytes -= packet.data.len();
            if packet.is_keyframe {
                self.keyframe_index.remove(&packet.pts);
            }
            self.base_offset += 1;
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
        let start_index = self
            .keyframe_index
            .range(..=start_pts)
            .last()
            .map(|(_, &abs_idx)| abs_idx.saturating_sub(self.base_offset))
            .unwrap_or(0);
        let remaining = self.packets.len().saturating_sub(start_index);
        let mut result = Vec::with_capacity(remaining);
        result.extend(self.packets.iter().skip(start_index).cloned());
        Ok(result)
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
        self.base_offset = 0;
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
        let target_duration_qpc = (self.duration.as_secs_f64() * qpc_frequency() as f64)
            as i64;
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
