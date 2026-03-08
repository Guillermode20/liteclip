//! Auto-generated module
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use crate::encode::{EncodedPacket, StreamType};
use anyhow::Result;
use bytes::Bytes;
use parking_lot::RwLock;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, trace, warn};

use super::functions::{h264_nal_type, hevc_nal_type, qpc_frequency};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodecKind {
    #[default]
    H264,
    Hevc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstVideoKind {
    H264Sps,
    HevcVps,
    Other,
}

impl FirstVideoKind {
    pub fn is_parameter_set(&self) -> bool {
        matches!(self, FirstVideoKind::H264Sps | FirstVideoKind::HevcVps)
    }
}

/// Thread-safe wrapper around ReplayBuffer
#[derive(Clone)]
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
    /// Push a batch of new packets into the buffer (acquires write lock once)
    pub fn push_batch(&self, packets: impl IntoIterator<Item = EncodedPacket>) {
        self.inner.write().push_batch(packets);
    }
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
    /// Soft clear - removes all packets but preserves keyframe tracking metadata
    pub fn soft_clear(&self) {
        self.inner.write().soft_clear();
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
    /// Check if buffer contains at least one keyframe (required for valid H.264 clip)
    pub fn has_keyframe(&self) -> bool {
        self.inner.read().has_keyframe()
    }
}
/// In-memory ring buffer for encoded packets
pub struct ReplayBuffer {
    /// Packet queue (oldest at front)
    pub(crate) packets: VecDeque<EncodedPacket>,
    /// Target duration
    duration: Duration,
    /// Cached target duration in QPC units (avoids per-push floating point multiplication)
    target_duration_qpc: i64,
    /// Max memory budget in bytes
    max_memory_bytes: usize,
    /// Keyframe index: VecDeque of (pts, absolute_index) pairs for O(1) front/back ops
    /// absolute_index is the global index (base_offset + relative index in packets)
    pub(crate) keyframe_index: VecDeque<(i64, usize)>,
    /// Number of packets evicted from front; used to compute relative indices
    base_offset: usize,
    /// Total bytes currently stored
    pub(crate) total_bytes: usize,
    /// Cached SPS (NAL type 7) for H.264 stream recovery after clear
    cached_sps: Option<Bytes>,
    /// Cached PPS (NAL type 8) for H.264 stream recovery after clear
    cached_pps: Option<Bytes>,
    /// Cached VPS (NAL type 32) for HEVC stream recovery after clear
    cached_vps: Option<Bytes>,
    /// Cached HEVC SPS (NAL type 33) for stream recovery after clear
    cached_hevc_sps: Option<Bytes>,
    /// Cached HEVC PPS (NAL type 34) for stream recovery after clear
    cached_hevc_pps: Option<Bytes>,
    /// First video packet index and kind tracking (avoids O(n) search in snapshot)
    first_video_info: Option<(usize, FirstVideoKind)>,
    /// Detected codec in the buffer (H.264 or HEVC)
    codec_kind: CodecKind,
}
impl ReplayBuffer {
    fn compute_target_duration_qpc(duration: Duration) -> i64 {
        (duration.as_secs_f64() * qpc_frequency() as f64) as i64
    }

    /// Create new replay buffer from configuration
    pub fn new(config: &crate::config::Config) -> Result<Self> {
        let duration = Duration::from_secs(config.general.replay_duration_secs as u64 + 1); // +1s for corrupted frame padding
        let target_duration_qpc = Self::compute_target_duration_qpc(duration);
        let effective_memory_limit_mb = config.effective_replay_memory_limit_mb();
        let max_memory_bytes = (effective_memory_limit_mb as usize).saturating_mul(1024 * 1024);
        debug!(
            "Creating ReplayBuffer: {} seconds, {} MB max (estimated payload {} MB)",
            duration.as_secs(),
            effective_memory_limit_mb,
            config.estimated_replay_storage_mb()
        );
        let estimated_packets = (duration.as_secs_f32() * 60.0).max(100.0) as usize; // estimate 60 fps + audio

        Ok(Self {
            packets: VecDeque::with_capacity(estimated_packets),
            duration,
            target_duration_qpc,
            max_memory_bytes,
            keyframe_index: VecDeque::with_capacity(estimated_packets / 30),
            base_offset: 0,
            total_bytes: 0,
            cached_sps: None,
            cached_pps: None,
            cached_vps: None,
            cached_hevc_sps: None,
            cached_hevc_pps: None,
            first_video_info: None,
            codec_kind: CodecKind::default(),
        })
    }
    /// Create a new replay buffer with explicit parameters
    pub fn with_params(duration: Duration, max_memory_mb: usize) -> Self {
        let target_duration_qpc = Self::compute_target_duration_qpc(duration);
        let max_memory_bytes = max_memory_mb.saturating_mul(1024 * 1024);
        debug!(
            "Creating ReplayBuffer: {} seconds, {} MB max",
            duration.as_secs(),
            max_memory_mb
        );
        let estimated_packets = (duration.as_secs_f32() * 60.0).max(100.0) as usize; // estimate 60 fps + audio

        Self {
            packets: VecDeque::with_capacity(estimated_packets),
            duration,
            target_duration_qpc,
            max_memory_bytes,
            keyframe_index: VecDeque::with_capacity(estimated_packets / 30), // assuming keyframe every 1-2 seconds
            base_offset: 0,
            total_bytes: 0,
            cached_sps: None,
            cached_pps: None,
            cached_vps: None,
            cached_hevc_sps: None,
            cached_hevc_pps: None,
            first_video_info: None,
            codec_kind: CodecKind::default(),
        }
    }
    /// Push a batch of new packets into the buffer
    ///
    /// This is more efficient than calling push() multiple times, as it evaluates
    /// eviction criteria only once after all packets are added.
    pub fn push_batch(&mut self, packets: impl IntoIterator<Item = EncodedPacket>) {
        let mut added_count = 0;
        for packet in packets {
            let packet_size = packet.data.len();
            added_count += 1;
            if matches!(packet.stream, StreamType::Video) {
                match h264_nal_type(packet.data.as_ref()) {
                    Some(7) => {
                        self.cached_sps = Some(packet.data.clone());
                        self.codec_kind = CodecKind::H264;
                        trace!("Cached H.264 SPS ({} bytes)", packet.data.len());
                    }
                    Some(8) => {
                        self.cached_pps = Some(packet.data.clone());
                        trace!("Cached H.264 PPS ({} bytes)", packet.data.len());
                    }
                    _ => {}
                }
                match hevc_nal_type(packet.data.as_ref()) {
                    Some(32) => {
                        self.cached_vps = Some(packet.data.clone());
                        self.codec_kind = CodecKind::Hevc;
                        trace!("Cached HEVC VPS ({} bytes)", packet.data.len());
                    }
                    Some(33) => {
                        self.cached_hevc_sps = Some(packet.data.clone());
                        trace!("Cached HEVC SPS ({} bytes)", packet.data.len());
                    }
                    Some(34) => {
                        self.cached_hevc_pps = Some(packet.data.clone());
                        trace!("Cached HEVC PPS ({} bytes)", packet.data.len());
                    }
                    _ => {}
                }
                if self.first_video_info.is_none() {
                    let kind = self.detect_first_video_kind(packet.data.as_ref());
                    self.first_video_info = Some((self.packets.len(), kind));
                }
            }
            if packet.is_keyframe {
                let absolute_index = self.base_offset + self.packets.len();
                self.keyframe_index.push_back((packet.pts, absolute_index));
                trace!(
                    "Added keyframe to index: pts={}, abs_idx={}, total_keyframes={}",
                    packet.pts,
                    absolute_index,
                    self.keyframe_index.len()
                );
            }
            self.total_bytes += packet_size;
            self.packets.push_back(packet);
        }
        if added_count == 0 {
            return;
        }
        let newest_pts = self.packets.back().map(|p| p.pts).unwrap();
        while !self.packets.is_empty() {
            let oldest_pts = self.packets.front().map(|p| p.pts).unwrap();
            let projected_span = newest_pts.saturating_sub(oldest_pts);
            if projected_span <= self.target_duration_qpc {
                break;
            }
            self.evict_oldest();
        }
        while self.total_bytes > self.max_memory_bytes && !self.packets.is_empty() {
            self.evict_oldest();
        }
        if self.packets.len() % 100 < added_count {
            trace!(
                "Buffer: {} packets, {} bytes, {} keyframes",
                self.packets.len(),
                self.total_bytes,
                self.keyframe_index.len()
            );
        }
    }
    /// Push a new packet into the buffer (evicts old if needed based on duration)
    ///
    /// Uses atomic eviction - calculates and executes eviction in a single loop
    /// to avoid TOCTOU race conditions where packets could be added between
    /// calculation and eviction.
    pub fn push(&mut self, packet: EncodedPacket) {
        let packet_size = packet.data.len();
        if packet_size > self.max_memory_bytes {
            warn!(
                "Dropping oversized packet ({} bytes) exceeding buffer memory cap ({} bytes)",
                packet_size, self.max_memory_bytes
            );
            return;
        }
        while !self.packets.is_empty() {
            let oldest_pts = self.packets.front().map(|p| p.pts).unwrap_or(packet.pts);
            let projected_span = packet.pts.saturating_sub(oldest_pts);
            if projected_span <= self.target_duration_qpc {
                break;
            }
            self.evict_oldest();
        }
        while self.total_bytes + packet_size > self.max_memory_bytes && !self.packets.is_empty() {
            self.evict_oldest();
        }
        if matches!(packet.stream, StreamType::Video) {
            match h264_nal_type(packet.data.as_ref()) {
                Some(7) => {
                    self.cached_sps = Some(packet.data.clone());
                    self.codec_kind = CodecKind::H264;
                    trace!("Cached H.264 SPS ({} bytes)", packet.data.len());
                }
                Some(8) => {
                    self.cached_pps = Some(packet.data.clone());
                    trace!("Cached H.264 PPS ({} bytes)", packet.data.len());
                }
                _ => {}
            }
            match hevc_nal_type(packet.data.as_ref()) {
                Some(32) => {
                    self.cached_vps = Some(packet.data.clone());
                    self.codec_kind = CodecKind::Hevc;
                    trace!("Cached HEVC VPS ({} bytes)", packet.data.len());
                }
                Some(33) => {
                    self.cached_hevc_sps = Some(packet.data.clone());
                    trace!("Cached HEVC SPS ({} bytes)", packet.data.len());
                }
                Some(34) => {
                    self.cached_hevc_pps = Some(packet.data.clone());
                    trace!("Cached HEVC PPS ({} bytes)", packet.data.len());
                }
                _ => {}
            }
            if self.first_video_info.is_none() {
                let kind = self.detect_first_video_kind(packet.data.as_ref());
                self.first_video_info = Some((self.packets.len(), kind));
            }
        }
        if packet.is_keyframe {
            let absolute_index = self.base_offset + self.packets.len();
            self.keyframe_index.push_back((packet.pts, absolute_index));
            trace!(
                "Added keyframe to index: pts={}, abs_idx={}, total_keyframes={}",
                packet.pts,
                absolute_index,
                self.keyframe_index.len()
            );
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
            let was_keyframe = packet.is_keyframe;
            let packet_pts = packet.pts;
            let was_video = matches!(packet.stream, StreamType::Video);
            self.total_bytes -= packet.data.len();
            self.base_offset += 1;
            trace!(
                "evict_oldest: packet_pts={}, was_keyframe={}, base_offset={}, keyframe_index_len_before={}",
                packet_pts, was_keyframe, self.base_offset - 1, self.keyframe_index.len()
            );
            while let Some(&(_, abs_idx)) = self.keyframe_index.front() {
                if abs_idx < self.base_offset {
                    trace!(
                        "  removing keyframe with abs_idx={} (base_offset={})",
                        abs_idx,
                        self.base_offset
                    );
                    self.keyframe_index.pop_front();
                } else {
                    break;
                }
            }
            if was_video {
                self.first_video_info = self.find_next_first_video_info();
            } else {
                self.first_video_info = self
                    .first_video_info
                    .map(|(idx, kind)| (idx.saturating_sub(1), kind));
            }
            trace!(
                "  after eviction: base_offset={}, keyframe_index_len_after={}",
                self.base_offset,
                self.keyframe_index.len()
            );
        }
    }

    fn find_next_first_video_info(&self) -> Option<(usize, FirstVideoKind)> {
        for (idx, packet) in self.packets.iter().enumerate() {
            if matches!(packet.stream, StreamType::Video) {
                let kind = self.detect_first_video_kind(packet.data.as_ref());
                return Some((idx, kind));
            }
        }
        None
    }

    fn detect_first_video_kind(&self, data: &[u8]) -> FirstVideoKind {
        if matches!(h264_nal_type(data), Some(7)) {
            return FirstVideoKind::H264Sps;
        }
        if matches!(hevc_nal_type(data), Some(32)) {
            return FirstVideoKind::HevcVps;
        }
        FirstVideoKind::Other
    }
    /// Get a snapshot of all packets (cheap clone via Bytes)
    ///
    /// If the buffer was cleared and has cached parameter sets, prepends them to ensure
    /// the stream is decodable. For H.264, prepends SPS/PPS. For HEVC, prepends VPS/SPS/PPS.
    pub fn snapshot(&self) -> Result<Vec<EncodedPacket>> {
        let mut result = Vec::with_capacity(self.packets.len() + 3);
        let first_video_is_param_set = self
            .first_video_info
            .map(|(_, kind)| kind.is_parameter_set())
            .unwrap_or(true);
        if !first_video_is_param_set {
            let first_video_pts = self
                .first_video_info
                .and_then(|(idx, _)| self.packets.get(idx).map(|p| p.pts))
                .unwrap_or(0);
            match self.codec_kind {
                CodecKind::H264 => {
                    if let Some(ref sps_data) = self.cached_sps {
                        result.push(EncodedPacket {
                            data: sps_data.clone(),
                            pts: first_video_pts,
                            dts: first_video_pts,
                            stream: StreamType::Video,
                            is_keyframe: false,
                            resolution: None,
                        });
                        trace!("Prepended cached H.264 SPS to snapshot");
                    }
                    if let Some(ref pps_data) = self.cached_pps {
                        result.push(EncodedPacket {
                            data: pps_data.clone(),
                            pts: first_video_pts,
                            dts: first_video_pts,
                            stream: StreamType::Video,
                            is_keyframe: false,
                            resolution: None,
                        });
                        trace!("Prepended cached H.264 PPS to snapshot");
                    }
                }
                CodecKind::Hevc => {
                    if let Some(ref vps_data) = self.cached_vps {
                        result.push(EncodedPacket {
                            data: vps_data.clone(),
                            pts: first_video_pts,
                            dts: first_video_pts,
                            stream: StreamType::Video,
                            is_keyframe: false,
                            resolution: None,
                        });
                        trace!("Prepended cached HEVC VPS to snapshot");
                    }
                    if let Some(ref sps_data) = self.cached_hevc_sps {
                        result.push(EncodedPacket {
                            data: sps_data.clone(),
                            pts: first_video_pts,
                            dts: first_video_pts,
                            stream: StreamType::Video,
                            is_keyframe: false,
                            resolution: None,
                        });
                        trace!("Prepended cached HEVC SPS to snapshot");
                    }
                    if let Some(ref pps_data) = self.cached_hevc_pps {
                        result.push(EncodedPacket {
                            data: pps_data.clone(),
                            pts: first_video_pts,
                            dts: first_video_pts,
                            stream: StreamType::Video,
                            is_keyframe: false,
                            resolution: None,
                        });
                        trace!("Prepended cached HEVC PPS to snapshot");
                    }
                }
            }
        }
        result.extend(self.packets.iter().cloned());
        Ok(result)
    }
    /// Get packets from timestamp to now
    ///
    /// Finds the nearest keyframe at or before start_pts and returns all packets
    /// from that point forward. This ensures the video can be decoded properly.
    /// Prepends cached parameter sets if the buffer was cleared.
    pub fn snapshot_from(&self, start_pts: i64) -> Result<Vec<EncodedPacket>> {
        let start_index = self.find_keyframe_index_before(start_pts).unwrap_or(0);
        let remaining = self.packets.len().saturating_sub(start_index);
        let mut result = Vec::with_capacity(remaining + 3);
        let packets_slice: Vec<EncodedPacket> =
            self.packets.iter().skip(start_index).cloned().collect();
        let first_video_kind = packets_slice
            .iter()
            .find_map(|p| {
                if matches!(p.stream, StreamType::Video) {
                    Some(self.detect_first_video_kind(p.data.as_ref()))
                } else {
                    None
                }
            })
            .unwrap_or(FirstVideoKind::H264Sps);
        if !first_video_kind.is_parameter_set() {
            let first_video_pts = packets_slice
                .iter()
                .find_map(|p| {
                    if matches!(p.stream, StreamType::Video) {
                        Some(p.pts)
                    } else {
                        None
                    }
                })
                .unwrap_or(0);
            match self.codec_kind {
                CodecKind::H264 => {
                    if let Some(ref sps_data) = self.cached_sps {
                        result.push(EncodedPacket {
                            data: sps_data.clone(),
                            pts: first_video_pts,
                            dts: first_video_pts,
                            stream: StreamType::Video,
                            is_keyframe: false,
                            resolution: None,
                        });
                        trace!("Prepended cached H.264 SPS to snapshot_from");
                    }
                    if let Some(ref pps_data) = self.cached_pps {
                        result.push(EncodedPacket {
                            data: pps_data.clone(),
                            pts: first_video_pts,
                            dts: first_video_pts,
                            stream: StreamType::Video,
                            is_keyframe: false,
                            resolution: None,
                        });
                        trace!("Prepended cached H.264 PPS to snapshot_from");
                    }
                }
                CodecKind::Hevc => {
                    if let Some(ref vps_data) = self.cached_vps {
                        result.push(EncodedPacket {
                            data: vps_data.clone(),
                            pts: first_video_pts,
                            dts: first_video_pts,
                            stream: StreamType::Video,
                            is_keyframe: false,
                            resolution: None,
                        });
                        trace!("Prepended cached HEVC VPS to snapshot_from");
                    }
                    if let Some(ref sps_data) = self.cached_hevc_sps {
                        result.push(EncodedPacket {
                            data: sps_data.clone(),
                            pts: first_video_pts,
                            dts: first_video_pts,
                            stream: StreamType::Video,
                            is_keyframe: false,
                            resolution: None,
                        });
                        trace!("Prepended cached HEVC SPS to snapshot_from");
                    }
                    if let Some(ref pps_data) = self.cached_hevc_pps {
                        result.push(EncodedPacket {
                            data: pps_data.clone(),
                            pts: first_video_pts,
                            dts: first_video_pts,
                            stream: StreamType::Video,
                            is_keyframe: false,
                            resolution: None,
                        });
                        trace!("Prepended cached HEVC PPS to snapshot_from");
                    }
                }
            }
        }
        result.extend(packets_slice);
        Ok(result)
    }
    /// Find the relative index of the last keyframe at or before the given pts.
    /// Uses linear search from the end for correctness with VecDeque and non-monotonic PTS.
    fn find_keyframe_index_before(&self, target_pts: i64) -> Option<usize> {
        trace!(
            "find_keyframe_index_before: target_pts={}, base_offset={}, keyframe_index_len={}",
            target_pts,
            self.base_offset,
            self.keyframe_index.len()
        );
        let search_idx = match self
            .keyframe_index
            .binary_search_by(|&(pts, _)| pts.cmp(&target_pts))
        {
            Ok(i) => i,
            Err(i) => {
                if i == 0 {
                    trace!("  no keyframe found at or before target_pts");
                    return None;
                }
                i - 1
            }
        };

        for i in (0..=search_idx).rev() {
            let (pts, abs_idx) = self.keyframe_index[i];
            let relative_idx = abs_idx.saturating_sub(self.base_offset);
            if relative_idx < self.packets.len() {
                trace!(
                    "  found: pts={}, abs_idx={}, relative_idx={}",
                    pts,
                    abs_idx,
                    relative_idx
                );
                return Some(relative_idx);
            } else {
                trace!(
                    "  found but relative_idx={} out of bounds (packets_len={}), continuing search",
                    relative_idx,
                    self.packets.len()
                );
            }
        }

        trace!("  no keyframe found at or before target_pts (after bound checks)");
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
    /// Clear all packets (preserves cached parameter sets for stream recovery)
    pub fn clear(&mut self) {
        self.packets.clear();
        self.keyframe_index.clear();
        self.base_offset = 0;
        self.total_bytes = 0;
        self.first_video_info = None;
        debug!(
            "Buffer cleared (H.264 SPS: {}, PPS: {} | HEVC VPS: {}, SPS: {}, PPS: {})",
            self.cached_sps.is_some(),
            self.cached_pps.is_some(),
            self.cached_vps.is_some(),
            self.cached_hevc_sps.is_some(),
            self.cached_hevc_pps.is_some()
        );
    }
    /// Soft clear - removes all packets and resets all tracking state
    ///
    /// This clears packets, keyframe index, and resets base_offset to prevent
    /// stale indices from causing incorrect keyframe lookups after new packets
    /// are added. Cached parameter sets are preserved for stream recovery.
    pub fn soft_clear(&mut self) {
        self.packets.clear();
        self.keyframe_index.clear();
        self.base_offset = 0;
        self.total_bytes = 0;
        self.first_video_info = None;
        debug!(
            "Buffer soft-cleared (H.264 SPS: {}, PPS: {} | HEVC VPS: {}, SPS: {}, PPS: {})",
            self.cached_sps.is_some(),
            self.cached_pps.is_some(),
            self.cached_vps.is_some(),
            self.cached_hevc_sps.is_some(),
            self.cached_hevc_pps.is_some()
        );
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
        let first = self.packets.front().map(|p| p.pts).unwrap_or(0);
        let last = self.packets.back().map(|p| p.pts).unwrap_or(0);
        last.saturating_sub(first) >= self.target_duration_qpc
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
    /// Check if buffer contains at least one keyframe (required for valid H.264 clip)
    pub fn has_keyframe(&self) -> bool {
        !self.keyframe_index.is_empty()
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
