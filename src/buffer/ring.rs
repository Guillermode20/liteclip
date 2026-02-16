//! Ring Buffer Implementation
//!
//! Rolling window of encoded packets backed by Bytes crate for cheap cloning.
//! Thread-safe with parking_lot::RwLock for concurrent access.

use crate::encode::EncodedPacket;
use anyhow::Result;
use parking_lot::RwLock;
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tracing::{debug, trace};

/// Cached QPC frequency (queried once, reused everywhere)
fn qpc_frequency() -> i64 {
    static FREQ: OnceLock<i64> = OnceLock::new();
    *FREQ.get_or_init(|| {
        let mut freq = 10_000_000i64;
        unsafe {
            windows::Win32::System::Performance::QueryPerformanceFrequency(&mut freq)
                .expect("QueryPerformanceFrequency should never fail on supported Windows");
        }
        freq
    })
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
    inner: Arc<RwLock<ReplayBuffer>>,
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

impl Clone for SharedReplayBuffer {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

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
            keyframe_index: BTreeMap::new(),
            total_bytes: 0,
            base_offset: 0,
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
            keyframe_index: BTreeMap::new(),
            total_bytes: 0,
            base_offset: 0,
        }
    }

    /// Push a new packet into the buffer (evicts old if needed based on duration)
    pub fn push(&mut self, packet: EncodedPacket) {
        let packet_size = packet.data.len();

        // Calculate target duration in QPC units
        let target_duration_qpc = (self.duration.as_secs_f64() * qpc_frequency() as f64) as i64;

        // Evict based on duration: remove packets until we're under target duration
        while !self.packets.is_empty() {
            let oldest_pts = self.packets.front().map(|p| p.pts).unwrap_or(packet.pts);
            let projected_span = packet.pts.saturating_sub(oldest_pts);

            if projected_span <= target_duration_qpc {
                break;
            }
            self.evict_oldest();
        }

        // Memory cap as safety guard (in case of timestamp issues or config errors)
        while self.total_bytes + packet_size > self.max_memory_bytes && !self.packets.is_empty() {
            self.evict_oldest();
        }

        // Index keyframes using absolute index (base_offset + current length)
        if packet.is_keyframe {
            let abs_index = self.base_offset + self.packets.len();
            self.keyframe_index.insert(packet.pts, abs_index);
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

            // Remove from keyframe index if present
            if packet.is_keyframe {
                self.keyframe_index.remove(&packet.pts);
            }

            // Increment base_offset so absolute indices remain valid
            self.base_offset += 1;
        }
    }

    /// Get a snapshot of all packets (cheap clone via Bytes)
    pub fn snapshot(&self) -> Result<Vec<EncodedPacket>> {
        // Clone is cheap because Bytes is reference-counted
        // Pre-allocate with exact capacity to avoid reallocations
        let mut result = Vec::with_capacity(self.packets.len());
        result.extend(self.packets.iter().cloned());
        Ok(result)
    }

    /// Get packets from timestamp to now
    ///
    /// Finds the nearest keyframe at or before start_pts and returns all packets
    /// from that point forward. This ensures the video can be decoded properly.
    pub fn snapshot_from(&self, start_pts: i64) -> Result<Vec<EncodedPacket>> {
        // Find nearest keyframe at or before start_pts
        // The stored index is absolute, so subtract base_offset to get VecDeque position
        let start_index = self
            .keyframe_index
            .range(..=start_pts)
            .last()
            .map(|(_, &abs_idx)| abs_idx.saturating_sub(self.base_offset))
            .unwrap_or(0);

        // Pre-allocate with exact capacity to avoid reallocations
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

        // Estimate duration based on packet timestamps
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode::StreamType;
    use bytes::Bytes;

    fn create_test_packet(pts: i64, is_keyframe: bool, size: usize) -> EncodedPacket {
        EncodedPacket {
            data: Bytes::from(vec![0u8; size]),
            pts,
            dts: pts,
            is_keyframe,
            stream: StreamType::Video,
            resolution: None,
        }
    }

    #[test]
    fn test_buffer_push_and_snapshot() {
        let mut buffer = ReplayBuffer::with_params(Duration::from_secs(120), 512);

        // Push 10 keyframes
        for i in 0..10 {
            let packet = create_test_packet(i * 1_000_000, true, 1024);
            buffer.push(packet);
        }

        assert_eq!(buffer.packets.len(), 10);
        assert_eq!(buffer.keyframe_index.len(), 10);

        let snapshot = buffer.snapshot().unwrap();
        assert_eq!(snapshot.len(), 10);
    }

    #[test]
    fn test_memory_budget_enforcement() {
        let mut buffer = ReplayBuffer::with_params(Duration::from_secs(120), 1); // 1 MB max

        // Push packets that exceed memory budget
        for i in 0..100 {
            let packet = create_test_packet(i * 1_000_000, i % 10 == 0, 50_000); // 50KB each
            buffer.push(packet);
        }

        // Should have evicted old packets to stay under 1MB
        assert!(buffer.total_bytes <= buffer.max_memory_bytes);
        assert!(buffer.packets.len() < 100);
    }

    #[test]
    fn test_keyframe_seeking() {
        let mut buffer = ReplayBuffer::with_params(Duration::from_secs(120), 512);

        // Push interleaved keyframes and delta frames
        for i in 0..30 {
            let is_keyframe = i % 5 == 0; // Keyframe every 5 frames
            let packet = create_test_packet(i * 1_000_000, is_keyframe, 1024);
            buffer.push(packet);
        }

        // Request from a non-keyframe position (should return from nearest keyframe)
        let snapshot = buffer.snapshot_from(12_000_000).unwrap();
        // Should start from keyframe at 10_000_000 (index 2)
        assert!(!snapshot.is_empty());
    }

    #[test]
    fn test_snapshot_cheap_clone() {
        let mut buffer = ReplayBuffer::with_params(Duration::from_secs(120), 512);

        // Push a large packet
        let large_data = vec![0u8; 1_000_000]; // 1MB
        let packet = EncodedPacket {
            data: Bytes::from(large_data),
            pts: 0,
            dts: 0,
            is_keyframe: true,
            stream: StreamType::Video,
            resolution: None,
        };
        buffer.push(packet);

        // Snapshot should be cheap (just ref counting, not copying)
        let snapshot = buffer.snapshot().unwrap();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].data.len(), 1_000_000);
    }

    #[test]
    fn test_clear() {
        let mut buffer = ReplayBuffer::with_params(Duration::from_secs(120), 512);

        for i in 0..10 {
            buffer.push(create_test_packet(i * 1_000_000, true, 1024));
        }

        buffer.clear();

        assert!(buffer.packets.is_empty());
        assert!(buffer.keyframe_index.is_empty());
        assert_eq!(buffer.total_bytes, 0);
        assert_eq!(buffer.base_offset, 0);
    }

    #[test]
    fn test_eviction_keyframe_index_correctness() {
        // Push enough packets to trigger many evictions, then verify snapshot_from still works
        let mut buffer = ReplayBuffer::with_params(Duration::from_secs(120), 1); // 1 MB

        let mut last_keyframe_pts = 0i64;
        for i in 0..200 {
            let is_kf = i % 10 == 0;
            let pts = i * 1_000_000;
            if is_kf {
                last_keyframe_pts = pts;
            }
            buffer.push(create_test_packet(pts, is_kf, 10_000)); // 10KB each → ~100 fit in 1MB
        }

        // Buffer should have evicted many packets
        assert!(buffer.packets.len() < 200);
        assert!(buffer.base_offset > 0);

        // snapshot_from with the last known keyframe should still return packets
        let snap = buffer.snapshot_from(last_keyframe_pts).unwrap();
        assert!(!snap.is_empty());
        // First packet in snapshot should be at or before the requested pts
        assert!(snap[0].pts <= last_keyframe_pts + 10_000_000); // within one keyframe interval
    }
}
