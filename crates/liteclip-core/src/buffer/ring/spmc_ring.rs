//! SPMC replay buffer: atomic write index + per-slot mutexes
//!
//! Stores encoded video and audio packets for the replay ring.
//!
//! # Design
//!
//! Single-producer / multi-consumer (SPMC) **ring addressing** uses an atomic write index
//! (`fetch_add`). Each slot still holds the packet behind a **per-slot `Mutex`** (and codec
//! parameter metadata uses a small shared mutex until cached). That is **not** a lock-free
//! push in the strict sense; it avoids one global queue lock while keeping snapshots safe.
//!
//! Until `param_cache_complete` is set, **video** pushes also take `param_cache` to parse and
//! store VPS/SPS/PPS — if profiling shows contention, consider supplying parameter sets from
//! the encoder init path instead of NAL scanning here.
//!
//! # Features
//!
//! - Push path: atomic slot selection + fine-grained slot locks (typical O(1) aside from lock hold time)
//! - Snapshots walk the ring with `try_lock` on slots (non-blocking where possible; see implementation)
//! - Parameter set caching (SPS/PPS/VPS) for proper clip decoding
//! - Keyframe tracking for seekable clips
//! - Memory and duration-based eviction
//! - Proactive eviction at 80% memory watermark to prevent mutex storms
//! - Batch eviction to reduce lock contention
//!
//! # Thread Safety
//!
//! The buffer is designed for the SPMC pattern:
//! - Push operations are thread-safe from a single producer
//! - Snapshot operations can be called from multiple consumers
//! - Clear operations are safe but should be coordinated
//!
//! # Example
//!
//! ```no_run
//! use liteclip_core::buffer::ring::LockFreeReplayBuffer;
//! use liteclip_core::config::Config;
//!
//! let config = Config::default();
//! let buffer = LockFreeReplayBuffer::new(&config).unwrap();
//! ```

use crate::buffer::BufferResult;
use crate::encode::{EncodedPacket, StreamType};
use bytes::Bytes;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, trace, warn};

use super::functions::{h264_nal_type, hevc_nal_type, qpc_frequency};
use super::types::BufferStats;

/// Memory usage percentage at which proactive eviction begins.
/// This prevents sudden mutex storms when memory hits 100% by smoothing
/// the eviction load across multiple push operations.
const PROACTIVE_EVICTION_WATERMARK: f32 = 0.80;

/// Number of slots to evict in a single batch.
/// Batch eviction reduces mutex contention by acquiring multiple locks
/// in a tight loop rather than with the full push path overhead between each.
const EVICTION_BATCH_SIZE: usize = 8;

/// Maximum bytes that can be pinned by in-flight snapshots.
/// When exceeded, snapshot_from returns an error to prevent unbounded memory growth.
/// Set to 50% of max_memory_bytes or 512MB minimum.
const MAX_OUTSTANDING_SNAPSHOT_BYTES: usize = 512 * 1024 * 1024; // 512MB default max

/// Threshold ratio for outstanding snapshot bytes warning.
const OUTSTANDING_SNAPSHOT_WARNING_RATIO: f32 = 0.75;

/// Cached codec parameter sets.
///
/// Stores H.264 SPS/PPS or HEVC VPS/SPS/PPS for inclusion in clip exports.
/// This ensures clips are playable even if the parameter sets were generated
/// before the clip start time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodecKind {
    /// H.264/AVC codec.
    #[default]
    H264,
    /// HEVC/H.265 codec.
    Hevc,
}

#[derive(Default)]
struct ParameterCache {
    codec_kind: CodecKind,
    h264_sps: Option<Bytes>,
    h264_pps: Option<Bytes>,
    hevc_vps: Option<Bytes>,
    hevc_sps: Option<Bytes>,
    hevc_pps: Option<Bytes>,
}

/// Identifies the first video NAL unit type.
///
/// Used to detect whether the first video packet is a parameter set
/// (SPS for H.264, VPS for HEVC) or regular encoded data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstVideoKind {
    /// H.264 Sequence Parameter Set (NAL type 7).
    H264Sps,
    /// HEVC Video Parameter Set (NAL type 32).
    HevcVps,
    /// Regular encoded data (not a parameter set).
    Other,
}

impl FirstVideoKind {
    /// Checks if this represents a parameter set.
    ///
    /// # Returns
    ///
    /// `true` if this is a parameter set (SPS or VPS).
    #[must_use]
    pub fn is_parameter_set(&self) -> bool {
        matches!(self, Self::H264Sps | Self::HevcVps)
    }
}

/// Wrapper for snapshot results that tracks outstanding bytes.
///
/// When a snapshot is created, the total bytes in the snapshot are added to
/// `outstanding_snapshot_bytes`. When this wrapper is dropped, the bytes are
/// subtracted, providing visibility into memory pinned by in-flight snapshots.
pub struct SnapshotBytes {
    inner: Arc<LockFreeInner>,
    bytes: usize,
}

impl SnapshotBytes {
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    fn new(inner: Arc<LockFreeInner>, bytes: usize) -> Self {
        if bytes > 0 {
            let current = inner
                .outstanding_snapshot_bytes
                .fetch_add(bytes, Ordering::Relaxed);
            let new_total = current.saturating_add(bytes);

            // Warn if approaching limit
            #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
            let warning_threshold = (inner.max_memory_bytes as f64
                * OUTSTANDING_SNAPSHOT_WARNING_RATIO as f64)
                as usize;
            if new_total > warning_threshold && current <= warning_threshold {
                warn!(
                    "Snapshot memory approaching limit: {:.1}MB / {:.1}MB",
                    new_total as f64 / 1_048_576.0,
                    inner.max_memory_bytes as f64 / 1_048_576.0
                );
            }
        }
        Self { inner, bytes }
    }
}

impl Drop for SnapshotBytes {
    fn drop(&mut self) {
        if self.bytes > 0 {
            self.inner
                .outstanding_snapshot_bytes
                .fetch_sub(self.bytes, Ordering::Relaxed);
        }
    }
}

/// A snapshot of encoded packets with memory tracking.
///
/// This wrapper tracks the total bytes in the snapshot and decrements
/// the outstanding count when dropped, providing visibility into memory
/// pinned by in-flight snapshots.
pub struct TrackedSnapshot {
    packets: Vec<EncodedPacket>,
    _tracker: SnapshotBytes,
}

impl TrackedSnapshot {
    fn new(packets: Vec<EncodedPacket>, inner: Arc<LockFreeInner>) -> Self {
        let bytes = packets.iter().map(|p| p.data.len()).sum();
        let tracker = SnapshotBytes::new(inner, bytes);
        Self {
            packets,
            _tracker: tracker,
        }
    }

    #[must_use]
    pub fn into_inner(self) -> Vec<EncodedPacket> {
        self.packets
    }

    #[must_use]
    pub fn as_slice(&self) -> &[EncodedPacket] {
        &self.packets
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.packets.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.packets.is_empty()
    }
}

impl std::ops::Deref for TrackedSnapshot {
    type Target = [EncodedPacket];

    fn deref(&self) -> &Self::Target {
        &self.packets
    }
}

impl IntoIterator for TrackedSnapshot {
    type Item = EncodedPacket;
    type IntoIter = std::vec::IntoIter<EncodedPacket>;

    fn into_iter(self) -> Self::IntoIter {
        self.packets.into_iter()
    }
}

/// Replay buffer ring: atomic write cursor with mutex-protected slots.
///
/// See the [module-level description](self) for the locking model.
///
/// # Thread Safety
///
/// - Push: intended for **one** producer thread (encoder / buffer writer).
/// - Snapshot / clear: multiple consumer threads; see `try_lock` behavior on slots.
/// - `Clone` is cheap (shallow `Arc` of inner state).
#[derive(Clone)]
pub struct LockFreeReplayBuffer {
    inner: Arc<LockFreeInner>,
}

struct LockFreeInner {
    slots: Box<[Slot]>,
    capacity: usize,
    mask: usize,
    max_duration_qpc: i64,
    write_idx: AtomicUsize,
    evict_frontier: AtomicUsize,
    max_memory_bytes: usize,
    total_bytes: AtomicUsize,
    keyframe_count: AtomicUsize,
    newest_pts: AtomicI64,
    param_cache: std::sync::Mutex<ParameterCache>,
    param_cache_complete: std::sync::atomic::AtomicBool,
    param_cache_pushes_since_complete: AtomicUsize,
    first_video_info: std::sync::Mutex<Option<(usize, FirstVideoKind)>>,
    outstanding_snapshot_bytes: AtomicUsize,
}

#[repr(align(64))]
struct Slot {
    packet: parking_lot::Mutex<Option<EncodedPacket>>,
}

impl Slot {
    const fn new() -> Self {
        Self {
            packet: parking_lot::Mutex::new(None),
        }
    }
}

impl LockFreeReplayBuffer {
    /// Creates a new lock-free replay buffer.
    ///
    /// # Arguments
    ///
    /// * `config` - Configuration with replay duration and memory limits.
    ///
    /// # Errors
    ///
    /// Returns an error if the buffer cannot be initialized with the given configuration.
    ///
    /// # Returns
    ///
    /// A new LockFreeReplayBuffer instance.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use liteclip_core::buffer::ring::LockFreeReplayBuffer;
    /// use liteclip_core::config::Config;
    ///
    /// let config = Config::default();
    /// let buffer = LockFreeReplayBuffer::new(&config).unwrap();
    /// ```
    #[allow(clippy::too_many_lines)]
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    pub fn new(config: &crate::config::Config) -> BufferResult<Self> {
        let duration = Duration::from_secs(config.general.replay_duration_secs as u64 + 1);
        let effective_memory_limit_mb = config.effective_replay_memory_limit_mb();
        let max_memory_bytes = (effective_memory_limit_mb as usize).saturating_mul(1024 * 1024);
        let qpc_freq = qpc_frequency().max(1);
        let max_duration_qpc =
            (config.general.replay_duration_secs.max(1) as i64).saturating_mul(qpc_freq);

        let video_packets_per_sec = config.video.framerate as f32;
        let audio_streams =
            (u8::from(config.audio.capture_system) + u8::from(config.audio.capture_mic)) as f32;
        let audio_packets_per_sec = audio_streams * 50.0;
        let packets_per_sec = video_packets_per_sec + audio_packets_per_sec;
        let estimated_packets = (duration.as_secs_f32() * packets_per_sec).max(100.0) as usize;
        let capacity = estimated_packets.next_power_of_two();
        let mask = capacity - 1;

        let mut slots = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            slots.push(Slot::new());
        }

        debug!(
            "Creating LockFreeReplayBuffer: {} seconds, {} MB max, {} slots ({} video + {} audio pps)",
            duration.as_secs(),
            effective_memory_limit_mb,
            capacity,
            video_packets_per_sec as u32,
            audio_packets_per_sec as u32
        );

        Ok(Self {
            inner: Arc::new(LockFreeInner {
                slots: slots.into_boxed_slice(),
                capacity,
                mask,
                max_duration_qpc,
                write_idx: AtomicUsize::new(0),
                evict_frontier: AtomicUsize::new(0),
                max_memory_bytes,
                total_bytes: AtomicUsize::new(0),
                keyframe_count: AtomicUsize::new(0),
                newest_pts: AtomicI64::new(0),
                param_cache: std::sync::Mutex::new(ParameterCache::default()),
                param_cache_complete: std::sync::atomic::AtomicBool::new(false),
                param_cache_pushes_since_complete: AtomicUsize::new(0),
                first_video_info: std::sync::Mutex::new(None),
                outstanding_snapshot_bytes: AtomicUsize::new(0),
            }),
        })
    }

    /// Pushes a batch of packets into the buffer.
    ///
    /// Thread-safe for single producer.
    ///
    /// # Arguments
    ///
    /// * `packets` - Iterator of encoded packets.
    #[allow(clippy::too_many_lines)]
    pub fn push_batch(&self, packets: impl IntoIterator<Item = EncodedPacket>) {
        for packet in packets {
            self.push_single(packet);
        }
    }

    /// Pushes a single packet into the buffer.
    ///
    /// Thread-safe for single producer. Uses atomic fetch_add for the write index.
    ///
    /// # Arguments
    ///
    /// * `packet` - The encoded packet to push.
    pub fn push(&self, packet: EncodedPacket) {
        self.push_single(packet);
    }
    /// Pushes a single encoded packet into the ring buffer.
    ///
    /// This method performs the following steps:
    /// 1. Caches parameter sets (SPS/PPS/VPS) if the packet contains them.
    /// 2. If it's the first video packet, detects its type (for clip start logic).
    /// 3. Atomically increments the write index and acquires a slot in the ring.
    /// 4. Replaces the old packet in the slot and updates buffer-wide stats (total bytes, keyframes).
    /// 5. Enforces the configured memory budget by evicting the oldest packets
    ///    via the `evict_frontier` index if necessary.
    ///
    /// # Thread Safety
    ///
    /// This is safe to call from a single producer thread. Multiple producers
    /// would require coordinating the `write_idx` increment to prevent races.
    fn push_single(&self, packet: EncodedPacket) {
        let inner = &self.inner;
        let packet_size = packet.data.len();
        let packet_pts = packet.pts;
        let is_keyframe = packet.is_keyframe;
        let stream_type = packet.stream;

        // Track pushes since param cache was completed for periodic refresh
        // Check BEFORE cache_parameter_sets to avoid race where we populate and immediately clear
        if inner.param_cache_complete.load(Ordering::Relaxed) {
            let pushes = inner
                .param_cache_pushes_since_complete
                .fetch_add(1, Ordering::Relaxed)
                + 1;
            // Periodic cache clear every 1000 pushes to prevent stale parameter sets
            // and to handle encoder reconfiguration (resolution changes, etc.)
            if pushes >= 1000 {
                self.clear_parameter_cache();
            }
        }

        self.cache_parameter_sets(&packet);

        if matches!(packet.stream, StreamType::Video) {
            let mut first_video_info = inner
                .first_video_info
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if first_video_info.is_none() {
                let kind = self.detect_first_video_kind(packet.data.as_ref());
                *first_video_info = Some((inner.write_idx.load(Ordering::Relaxed), kind));
            }
        }

        let write_idx = inner.write_idx.fetch_add(1, Ordering::Relaxed);
        let slot_idx = write_idx & inner.mask;
        let slot = &inner.slots[slot_idx];

        // Load total_bytes_before for logging BEFORE the lock block where fetch_add happens
        let total_bytes_before = inner.total_bytes.load(Ordering::Relaxed);

        // Track old packet for memory accounting - now includes new packet accounting inside lock
        let old_packet_size = {
            let mut packet_guard = slot.packet.lock();
            let old = packet_guard.take();
            let old_size = old.as_ref().map_or(0, |p| p.data.len());
            let old_was_keyframe = old.as_ref().is_some_and(|p| p.is_keyframe);
            *packet_guard = Some(packet);
            // Account for new packet bytes immediately, inside the lock.
            // Use Release ordering to "publish" the packet write to other threads.
            inner.total_bytes.fetch_add(packet_size, Ordering::Release);
            if old.is_some() {
                // Release ordering ensures the packet removal is visible before counter update
                inner.total_bytes.fetch_sub(old_size, Ordering::Release);
                if old_was_keyframe {
                    inner.keyframe_count.fetch_sub(1, Ordering::Release);
                }
            }
            if is_keyframe {
                inner.keyframe_count.fetch_add(1, Ordering::Release);
            }
            old_size
        };

        inner.newest_pts.store(packet_pts, Ordering::Release);
        self.evict_packets_older_than(packet_pts);

        // Update evict_frontier to track the oldest valid packet after ring wrap.
        // After fetch_add, the next write will be at write_idx + 1.
        // The oldest valid packet is at (write_idx + 1) - capacity when buffer is full.
        // Without this, memory eviction would evict NEW packets instead of old ones.
        let next_write_idx = write_idx + 1;
        if next_write_idx > inner.capacity {
            let oldest_valid = next_write_idx - inner.capacity;
            let current_frontier = inner.evict_frontier.load(Ordering::Relaxed);
            if current_frontier < oldest_valid {
                inner.evict_frontier.store(oldest_valid, Ordering::Release);
                trace!(
                    "Ring wrap: write_idx={}, capacity={}, evict_frontier {}->{}",
                    write_idx,
                    inner.capacity,
                    current_frontier,
                    oldest_valid
                );
            }
        }

        // Log every 100 video packets for memory tracking
        if matches!(stream_type, StreamType::Video) && write_idx % 100 == 0 {
            let evict_frontier = inner.evict_frontier.load(Ordering::Relaxed);
            let packet_count = write_idx.saturating_sub(evict_frontier);
            debug!(
                "Buffer push[{}]: stream={:?}, pkt_size={}KB, old_pkt_size={}KB, total={:.1}MB/{:.1}MB ({:.0}%), packets={}, write_idx={}, evict_frontier={}",
                write_idx,
                stream_type,
                packet_size / 1024,
                old_packet_size / 1024,
                total_bytes_before as f64 / 1_048_576.0,
                inner.max_memory_bytes as f64 / 1_048_576.0,
                (total_bytes_before as f64 / inner.max_memory_bytes as f64 * 100.0).min(999.0),
                packet_count,
                write_idx,
                evict_frontier
            );
        }

        // Log first few microphone packets to verify they reach the buffer
        if matches!(stream_type, StreamType::Microphone) && write_idx <= 5 {
            info!(
                "Buffer push[{}]: microphone packet stored, size={}B, pts={}",
                write_idx, packet_size, packet_pts
            );
        }

        // Enforce memory budget: if total_bytes exceeds max_memory_bytes, proactively
        // evict the oldest packets (those the ring would discard next anyway) until we
        // are back under the limit.  This prevents the initial fill phase from growing
        // RAM unboundedly when the configured bitrate generates packets larger than the
        // packet-count estimate assumed when sizing the ring capacity.
        //
        // We use two eviction triggers:
        // 1. Proactive eviction at 80% watermark - smooths eviction load and prevents
        //    sudden mutex storms when memory hits 100%
        // 2. Hard eviction at 100% - ensures memory stays bounded
        //
        // Batch eviction acquires multiple slot locks in a tight loop to reduce
        // contention compared to one-by-one eviction with full push path overhead.
        if inner.max_memory_bytes > 0 {
            // Use Acquire ordering to see latest writes from producer thread
            let current_total = inner.total_bytes.load(Ordering::Acquire);
            let memory_ratio = current_total as f32 / inner.max_memory_bytes as f32;

            // Determine if we need eviction and how aggressively
            let needs_eviction = memory_ratio > PROACTIVE_EVICTION_WATERMARK;

            if needs_eviction {
                let mut eviction_count = 0usize;
                let mut evicted_bytes = 0usize;
                let mut evicted_keyframes = 0usize;
                let start_total = current_total;
                let mut stopped_at_head = false;

                // Calculate target memory level:
                // - If above 100%, evict until below max_memory_bytes
                // - If above 80% but below 100%, evict a small batch to smooth load
                let target_ratio = if memory_ratio > 1.0 {
                    1.0 // Hard eviction: get below 100%
                } else {
                    PROACTIVE_EVICTION_WATERMARK - 0.05 // Proactive: evict to ~75%
                };
                let target_bytes = (inner.max_memory_bytes as f32 * target_ratio) as usize;

                // Batch eviction loop - Acquire to see latest counter updates
                while inner.total_bytes.load(Ordering::Acquire) > target_bytes {
                    // Collect a batch of slots to evict
                    let mut batch_evicted = 0usize;

                    for _ in 0..EVICTION_BATCH_SIZE {
                        let evict = inner.evict_frontier.load(Ordering::Relaxed);
                        if evict >= write_idx {
                            stopped_at_head = true;
                            warn!(
                                "Eviction: evict_frontier={} >= write_idx={}; no older slots to free",
                                evict, write_idx
                            );
                            break;
                        }

                        let evict_slot_idx = evict & inner.mask;
                        let slot = &inner.slots[evict_slot_idx];

                        // Acquire lock and evict packet
                        {
                            let mut guard = slot.packet.lock();
                            if let Some(old) = guard.take() {
                                let old_len = old.data.len();
                                // Release ordering ensures packet removal is visible before counter update
                                inner.total_bytes.fetch_sub(old_len, Ordering::Release);
                                if old.is_keyframe {
                                    inner.keyframe_count.fetch_sub(1, Ordering::Release);
                                    evicted_keyframes += 1;
                                }
                                evicted_bytes += old_len;
                                eviction_count += 1;
                                batch_evicted += 1;
                            } else {
                                trace!(
                                    "Eviction slot empty: evict_frontier={}, slot_idx={}",
                                    evict,
                                    evict_slot_idx
                                );
                            }
                        }
                        inner.evict_frontier.fetch_add(1, Ordering::Release);
                    }

                    // If we couldn't evict any packets in this batch, stop
                    if batch_evicted == 0 {
                        break;
                    }

                    // For proactive eviction, one batch is usually enough
                    if memory_ratio <= 1.0 && eviction_count >= EVICTION_BATCH_SIZE {
                        break;
                    }
                }

                if eviction_count > 0 {
                    let end_total = inner.total_bytes.load(Ordering::Relaxed);
                    let eviction_type = if memory_ratio > 1.0 {
                        "hard"
                    } else {
                        "proactive"
                    };
                    debug!(
                        "Buffer {} eviction: {} packets ({} keyframes) removed in batches of {}, {:.1}MB freed, total {:.1}MB -> {:.1}MB / {:.1}MB limit ({:.0}% -> {:.0}%)",
                        eviction_type,
                        eviction_count,
                        evicted_keyframes,
                        EVICTION_BATCH_SIZE,
                        evicted_bytes as f64 / 1_048_576.0,
                        start_total as f64 / 1_048_576.0,
                        end_total as f64 / 1_048_576.0,
                        inner.max_memory_bytes as f64 / 1_048_576.0,
                        (start_total as f64 / inner.max_memory_bytes as f64 * 100.0),
                        (end_total as f64 / inner.max_memory_bytes as f64 * 100.0)
                    );
                }

                // Oldest packets are gone but the packet we just wrote (or accounting skew) still exceeds the cap.
                if stopped_at_head
                    && inner.total_bytes.load(Ordering::Acquire) > inner.max_memory_bytes
                {
                    let slot_idx = write_idx & inner.mask;
                    let slot = &inner.slots[slot_idx];
                    let mut guard = slot.packet.lock();
                    if let Some(removed) = guard.take() {
                        let rm = removed.data.len();
                        // Release ordering ensures packet removal is visible before counter update
                        inner.total_bytes.fetch_sub(rm, Ordering::Release);
                        if removed.is_keyframe {
                            inner.keyframe_count.fetch_sub(1, Ordering::Release);
                        }
                        warn!(
                            "Buffer: dropped newest packet ({:.1}KB) to enforce memory cap {:.1}MB",
                            rm as f64 / 1024.0,
                            inner.max_memory_bytes as f64 / 1_048_576.0
                        );
                    }
                }
            }
        }
    }

    fn evict_packets_older_than(&self, newest_pts: i64) {
        let inner = &self.inner;
        if inner.max_duration_qpc <= 0 || newest_pts <= 0 {
            return;
        }

        let cutoff_pts = newest_pts.saturating_sub(inner.max_duration_qpc);
        let write_idx = inner.write_idx.load(Ordering::Acquire);
        let mut duration_evicted_packets = 0usize;
        let mut duration_evicted_bytes = 0usize;
        let mut duration_evicted_keyframes = 0usize;

        loop {
            let evict = inner.evict_frontier.load(Ordering::Acquire);
            if evict >= write_idx {
                break;
            }

            let slot_idx = evict & inner.mask;
            let slot = &inner.slots[slot_idx];
            let mut guard = slot.packet.lock();

            let should_evict = match guard.as_ref() {
                Some(packet) => packet.pts < cutoff_pts,
                None => true,
            };

            if !should_evict {
                break;
            }

            if let Some(old) = guard.take() {
                let old_len = old.data.len();
                // Release ordering ensures packet removal is visible before counter update
                inner.total_bytes.fetch_sub(old_len, Ordering::Release);
                if old.is_keyframe {
                    inner.keyframe_count.fetch_sub(1, Ordering::Release);
                    duration_evicted_keyframes += 1;
                }
                duration_evicted_bytes += old_len;
                duration_evicted_packets += 1;
            }
            drop(guard);
            inner.evict_frontier.fetch_add(1, Ordering::Release);
        }

        if duration_evicted_packets > 0 {
            debug!(
                "Buffer duration eviction: {} packets ({} keyframes), {:.1}MB freed, cutoff_pts={}, newest_pts={}, target_window={:.1}s",
                duration_evicted_packets,
                duration_evicted_keyframes,
                duration_evicted_bytes as f64 / 1_048_576.0,
                cutoff_pts,
                newest_pts,
                inner.max_duration_qpc as f64 / qpc_frequency().max(1) as f64
            );
        }
    }

    fn cache_parameter_sets(&self, packet: &EncodedPacket) {
        if !matches!(packet.stream, StreamType::Video) || packet.data.is_empty() {
            return;
        }

        if self.inner.param_cache_complete.load(Ordering::Relaxed) {
            return;
        }

        let inner = &self.inner;
        let data = packet.data.as_ref();

        let mut cache = inner.param_cache.lock().unwrap_or_else(|e| e.into_inner());

        // Early exit if we already have all parameters for current codec
        let has_all_params = match cache.codec_kind {
            CodecKind::H264 => cache.h264_sps.is_some() && cache.h264_pps.is_some(),
            CodecKind::Hevc => {
                cache.hevc_vps.is_some() && cache.hevc_sps.is_some() && cache.hevc_pps.is_some()
            }
        };
        if has_all_params {
            inner.param_cache_complete.store(true, Ordering::Release);
            return;
        }

        let mut i = 0;
        while i < data.len() {
            let start_code_len;
            if i + 4 <= data.len() && data[i..i + 4] == [0x00, 0x00, 0x00, 0x01] {
                start_code_len = 4;
            } else if i + 3 <= data.len() && data[i..i + 3] == [0x00, 0x00, 0x01] {
                start_code_len = 3;
            } else {
                i += 1;
                continue;
            }

            let nal_start = i + start_code_len;
            if nal_start >= data.len() {
                break;
            }

            let hevc_nal = (data[nal_start] >> 1) & 0x3f;
            let h264_nal = data[nal_start] & 0x1f;

            // Find NAL end
            let mut nal_end = nal_start + 1;
            while nal_end < data.len() {
                if (nal_end + 3 <= data.len() && data[nal_end..nal_end + 3] == [0x00, 0x00, 0x01])
                    || (nal_end + 4 <= data.len()
                        && data[nal_end..nal_end + 4] == [0x00, 0x00, 0x00, 0x01])
                {
                    break;
                }
                nal_end += 1;
            }

            let nal_data = Bytes::copy_from_slice(&data[i..nal_end]);

            let already_hevc =
                cache.hevc_vps.is_some() || cache.hevc_sps.is_some() || cache.hevc_pps.is_some();

            match hevc_nal {
                32 => {
                    cache.hevc_vps = Some(nal_data);
                    cache.codec_kind = CodecKind::Hevc;
                    trace!("Cached HEVC VPS ({} bytes)", nal_end - i);
                }
                33 => {
                    cache.hevc_sps = Some(nal_data);
                    cache.codec_kind = CodecKind::Hevc;
                    trace!("Cached HEVC SPS ({} bytes)", nal_end - i);
                }
                34 => {
                    cache.hevc_pps = Some(nal_data);
                    cache.codec_kind = CodecKind::Hevc;
                    trace!("Cached HEVC PPS ({} bytes)", nal_end - i);
                }
                _ => {
                    if !already_hevc {
                        match h264_nal {
                            7 => {
                                cache.h264_sps = Some(nal_data);
                                cache.codec_kind = CodecKind::H264;
                                trace!("Cached H.264 SPS ({} bytes)", nal_end - i);
                            }
                            8 => {
                                cache.h264_pps = Some(nal_data);
                                cache.codec_kind = CodecKind::H264;
                                trace!("Cached H.264 PPS ({} bytes)", nal_end - i);
                            }
                            _ => {}
                        }
                    }
                }
            }

            // Check if we have all parameters now
            let complete = match cache.codec_kind {
                CodecKind::H264 => cache.h264_sps.is_some() && cache.h264_pps.is_some(),
                CodecKind::Hevc => {
                    cache.hevc_vps.is_some() && cache.hevc_sps.is_some() && cache.hevc_pps.is_some()
                }
            };
            if complete {
                inner.param_cache_complete.store(true, Ordering::Release);
                return;
            }

            i = nal_end;
        }
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

    /// Gets a snapshot of all packets in the buffer.
    ///
    /// Returns all packets from the oldest to newest, including prepended
    /// parameter sets (SPS/PPS or VPS/SPS/PPS) if the first video packet
    /// is not a parameter set.
    ///
    /// # Errors
    ///
    /// Returns an error if snapshot creation fails (e.g., memory limit exceeded).
    ///
    /// # Returns
    ///
    /// Vector of all encoded packets in chronological order.
    ///
    /// # Thread Safety
    ///
    /// Safe to call from multiple consumer threads.
    #[allow(clippy::too_many_lines)]
    pub fn snapshot(&self) -> BufferResult<TrackedSnapshot> {
        let inner = &self.inner;
        let write_idx = inner.write_idx.load(Ordering::Acquire);

        if write_idx == 0 {
            return Ok(TrackedSnapshot::new(vec![], Arc::clone(inner)));
        }

        let capacity = inner.capacity;
        let evict_frontier = inner.evict_frontier.load(Ordering::Acquire);
        let start_idx = write_idx.saturating_sub(capacity).max(evict_frontier);
        let count = write_idx - start_idx;

        let mut result = Vec::with_capacity(count);

        for i in start_idx..write_idx {
            let slot_idx = i & inner.mask;
            let slot = &inner.slots[slot_idx];

            if let Some(packet_guard) = slot.packet.try_lock() {
                if let Some(ref packet) = *packet_guard {
                    // Zero-copy clone: Bytes uses Arc internally, so clone() just bumps
                    // the refcount. Ring eviction still works because the slot is cleared
                    // via guard.take(), and the snapshot's clone keeps data alive.
                    result.push(EncodedPacket {
                        data: packet.data.clone(),
                        pts: packet.pts,
                        dts: packet.dts,
                        stream: packet.stream,
                        is_keyframe: packet.is_keyframe,
                        resolution: packet.resolution,
                    });
                }
            }
        }

        let first_video_is_param_set = inner
            .first_video_info
            .lock()
            .unwrap()
            .map(|(_, kind)| kind.is_parameter_set())
            .unwrap_or(true);

        if !first_video_is_param_set && !result.is_empty() {
            let first_video_pts = result
                .iter()
                .find_map(|p| {
                    if matches!(p.stream, StreamType::Video) {
                        Some(p.pts)
                    } else {
                        None
                    }
                })
                .unwrap_or(0);

            let cache = inner.param_cache.lock().unwrap_or_else(|e| e.into_inner());
            let mut prepend = Vec::new();

            match cache.codec_kind {
                CodecKind::H264 => {
                    if let Some(ref sps_data) = cache.h264_sps {
                        prepend.push(EncodedPacket {
                            data: sps_data.clone(),
                            pts: first_video_pts,
                            dts: first_video_pts,
                            stream: StreamType::Video,
                            is_keyframe: false,
                            resolution: None,
                        });
                    }
                    if let Some(ref pps_data) = cache.h264_pps {
                        prepend.push(EncodedPacket {
                            data: pps_data.clone(),
                            pts: first_video_pts,
                            dts: first_video_pts,
                            stream: StreamType::Video,
                            is_keyframe: false,
                            resolution: None,
                        });
                    }
                }
                CodecKind::Hevc => {
                    if let Some(ref vps_data) = cache.hevc_vps {
                        prepend.push(EncodedPacket {
                            data: vps_data.clone(),
                            pts: first_video_pts,
                            dts: first_video_pts,
                            stream: StreamType::Video,
                            is_keyframe: false,
                            resolution: None,
                        });
                    }
                    if let Some(ref sps_data) = cache.hevc_sps {
                        prepend.push(EncodedPacket {
                            data: sps_data.clone(),
                            pts: first_video_pts,
                            dts: first_video_pts,
                            stream: StreamType::Video,
                            is_keyframe: false,
                            resolution: None,
                        });
                    }
                    if let Some(ref pps_data) = cache.hevc_pps {
                        prepend.push(EncodedPacket {
                            data: pps_data.clone(),
                            pts: first_video_pts,
                            dts: first_video_pts,
                            stream: StreamType::Video,
                            is_keyframe: false,
                            resolution: None,
                        });
                    }
                }
            }

            let mut final_result = Vec::with_capacity(prepend.len() + result.len());
            final_result.extend(prepend);
            final_result.extend(result);
            return Ok(TrackedSnapshot::new(final_result, Arc::clone(inner)));
        }

        Ok(TrackedSnapshot::new(result, Arc::clone(inner)))
    }

    /// Gets a snapshot starting from a specific PTS.
    ///
    /// Prefers the last keyframe at or before the requested PTS so the clip
    /// keeps the leading audio/video that would otherwise be dropped.
    /// Falls back to the first keyframe after the requested PTS only when no
    /// earlier keyframe is available.
    ///
    /// Uses a two-pass approach to avoid doubling memory:
    ///   Pass 1 — scan slot metadata (pts, keyframe, stream) without cloning.
    ///   Pass 2 — clone only the packets that belong in the final result.
    ///
    /// # Arguments
    ///
    /// * `start_pts` - The starting presentation timestamp (in stream timebase).
    ///
    /// # Errors
    ///
    /// Returns an error if snapshot creation fails or outstanding snapshot bytes exceed limit.
    ///
    /// # Returns
    ///
    /// Vector of encoded packets starting from a keyframe.
    ///
    /// # Thread Safety
    ///
    /// Safe to call from multiple consumer threads.
    #[allow(clippy::too_many_lines)]
    pub fn snapshot_from(&self, start_pts: i64) -> BufferResult<TrackedSnapshot> {
        let inner = &self.inner;
        let write_idx = inner.write_idx.load(Ordering::Acquire);

        if write_idx == 0 {
            return Ok(TrackedSnapshot::new(vec![], Arc::clone(inner)));
        }

        // Check outstanding snapshot bytes limit before proceeding
        let current_outstanding = inner.outstanding_snapshot_bytes.load(Ordering::Relaxed);
        let max_outstanding = inner.max_memory_bytes.min(MAX_OUTSTANDING_SNAPSHOT_BYTES);
        if current_outstanding >= max_outstanding {
            warn!(
                "snapshot_from rejected: outstanding snapshot bytes ({:.1}MB) exceeds limit ({:.1}MB)",
                current_outstanding as f64 / 1_048_576.0,
                max_outstanding as f64 / 1_048_576.0
            );
            return Err(crate::buffer::BufferError::SnapshotLimitExceeded {
                current: current_outstanding,
                limit: max_outstanding,
            });
        }

        let first_idx = write_idx.saturating_sub(inner.capacity);
        let evict_frontier = inner.evict_frontier.load(Ordering::Acquire);
        let first_idx = first_idx.max(evict_frontier);

        // Log buffer state at snapshot start
        let buffer_total_bytes = inner.total_bytes.load(Ordering::Relaxed);
        let buffer_packet_count = write_idx.saturating_sub(evict_frontier);
        debug!(
            "snapshot_from START: start_pts={}, write_idx={}, evict_frontier={}, buffer_packets={}, buffer_bytes={:.1}MB",
            start_pts,
            write_idx,
            evict_frontier,
            buffer_packet_count,
            buffer_total_bytes as f64 / 1_048_576.0
        );

        // ── Pass 1: scan-only (no clones) ──────────────────────────────────
        // Record lightweight metadata per slot to determine the keyframe
        // boundary and the video_start_pts, without cloning any packet data.
        struct SlotMeta {
            ring_idx: usize,
            pts: i64,
            is_keyframe: bool,
            stream: StreamType,
        }

        let capacity_estimate = write_idx.saturating_sub(first_idx);
        let mut metas: Vec<SlotMeta> = Vec::with_capacity(capacity_estimate);
        let mut first_keyframe_at_or_after: Option<usize> = None;
        let mut last_keyframe_at_or_before: Option<usize> = None;

        for i in first_idx..write_idx {
            let slot_idx = i & inner.mask;
            let slot = &inner.slots[slot_idx];

            if let Some(packet_guard) = slot.packet.try_lock() {
                if let Some(ref packet) = *packet_guard {
                    if packet.is_keyframe {
                        if packet.pts >= start_pts && first_keyframe_at_or_after.is_none() {
                            first_keyframe_at_or_after = Some(i);
                        }
                        if packet.pts <= start_pts {
                            last_keyframe_at_or_before = Some(i);
                        }
                    }
                    metas.push(SlotMeta {
                        ring_idx: i,
                        pts: packet.pts,
                        is_keyframe: packet.is_keyframe,
                        stream: packet.stream,
                    });
                }
            }
        }

        let start_idx = last_keyframe_at_or_before
            .or(first_keyframe_at_or_after)
            .unwrap_or(first_idx);

        // Derive video_start_pts from the metadata (no clones yet).
        let video_start_pts = metas
            .iter()
            .filter(|m| matches!(m.stream, StreamType::Video))
            .find(|m| m.ring_idx >= start_idx)
            .map(|m| m.pts)
            .unwrap_or(start_pts);

        // Collect included ring indices directly instead of using a bool Vec.
        // This avoids allocating a potentially large Vec<bool> (50K+ elements for large buffers).
        let mut included_indices: Vec<usize> = Vec::with_capacity(metas.len());
        for m in &metas {
            let dominated = m.ring_idx >= start_idx;
            let audio_in_range = !dominated
                && matches!(m.stream, StreamType::SystemAudio | StreamType::Microphone)
                && m.pts >= video_start_pts;
            if dominated || audio_in_range {
                included_indices.push(m.ring_idx);
            }
        }

        {
            let video_count = metas
                .iter()
                .filter(|m| matches!(m.stream, StreamType::Video))
                .count();
            let keyframe_count = metas.iter().filter(|m| m.is_keyframe).count();
            debug!(
                "snapshot_from: all_packets={} ({} video, {} keyframes), start_pts={}, start_idx={}, included={}",
                metas.len(),
                video_count,
                keyframe_count,
                start_pts,
                start_idx,
                included_indices.len()
            );
        }

        // Aggressively release metas — we no longer need it.
        metas.clear();
        metas.shrink_to_fit();
        drop(metas);

        // ── Pass 2: selective clone ────────────────────────────────────────
        // Use a sorted included_indices Vec for O(log n) lookup without large allocation
        // The Vec is already sorted from construction since we iterate in order
        let mut result: Vec<EncodedPacket> = Vec::with_capacity(included_indices.len());

        for i in first_idx..write_idx {
            // Binary search to check if this index is included
            if included_indices.binary_search(&i).is_err() {
                continue;
            }
            let slot_idx = i & inner.mask;
            let slot = &inner.slots[slot_idx];

            if let Some(packet_guard) = slot.packet.try_lock() {
                if let Some(ref packet) = *packet_guard {
                    // Zero-copy clone: Bytes uses Arc internally, so clone() just bumps
                    // the refcount. Ring eviction still works because the slot is cleared
                    // via guard.take(), and the snapshot's clone keeps data alive.
                    result.push(EncodedPacket {
                        data: packet.data.clone(),
                        pts: packet.pts,
                        dts: packet.dts,
                        stream: packet.stream,
                        is_keyframe: packet.is_keyframe,
                        resolution: packet.resolution,
                    });
                }
            }
        }

        // Release included_indices after loop completes.
        included_indices.clear();
        included_indices.shrink_to_fit();
        drop(included_indices);

        result.sort_by_key(|p| {
            (
                p.pts,
                match p.stream {
                    StreamType::Video => 0,
                    StreamType::SystemAudio => 1,
                    StreamType::Microphone => 2,
                },
            )
        });

        // Calculate result memory footprint
        let result_bytes: usize = result.iter().map(|p| p.data.len()).sum();
        let result_video = result
            .iter()
            .filter(|p| matches!(p.stream, StreamType::Video))
            .count();
        let result_audio = result.len().saturating_sub(result_video);

        // Log buffer state after snapshot to see if anything changed
        let buffer_bytes_after = inner.total_bytes.load(Ordering::Relaxed);
        let evict_after = inner.evict_frontier.load(Ordering::Relaxed);
        let write_idx_after = inner.write_idx.load(Ordering::Relaxed);

        debug!(
            "snapshot_from RESULT: result={} packets ({} video, {} audio), result_bytes={:.1}MB, buffer_after={:.1}MB, write_idx {}->{}, evict_frontier {}->{}",
            result.len(),
            result_video,
            result_audio,
            result_bytes as f64 / 1_048_576.0,
            buffer_bytes_after as f64 / 1_048_576.0,
            write_idx,
            write_idx_after,
            evict_frontier,
            evict_after
        );

        if !result.is_empty() {
            let first_video = result
                .iter()
                .find(|p| matches!(p.stream, StreamType::Video));
            if let Some(first_vid) = first_video {
                let first_is_keyframe = first_vid.is_keyframe;
                let first_data = first_vid.data.as_ref();
                let first_nal_is_vps = hevc_nal_type(first_data) == Some(32);
                let first_nal_is_sps = h264_nal_type(first_data) == Some(7);

                debug!(
                    "snapshot_from: first_video keyframe={}, nal_vps={}, nal_sps={}",
                    first_is_keyframe, first_nal_is_vps, first_nal_is_sps
                );

                if first_is_keyframe && !first_nal_is_vps && !first_nal_is_sps {
                    let cache = self
                        .inner
                        .param_cache
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    info!(
                        "snapshot_from: prepending param sets (codec={:?}, vps={}, sps={}, pps={})",
                        cache.codec_kind,
                        cache.hevc_vps.is_some(),
                        cache.hevc_sps.is_some(),
                        cache.hevc_pps.is_some()
                    );
                    let first_pts = first_vid.pts;
                    let mut prepend = Vec::new();

                    match cache.codec_kind {
                        CodecKind::H264 => {
                            if let Some(ref sps_data) = cache.h264_sps {
                                prepend.push(EncodedPacket {
                                    data: sps_data.clone(),
                                    pts: first_pts,
                                    dts: first_pts,
                                    stream: StreamType::Video,
                                    is_keyframe: false,
                                    resolution: None,
                                });
                            }
                            if let Some(ref pps_data) = cache.h264_pps {
                                prepend.push(EncodedPacket {
                                    data: pps_data.clone(),
                                    pts: first_pts,
                                    dts: first_pts,
                                    stream: StreamType::Video,
                                    is_keyframe: false,
                                    resolution: None,
                                });
                            }
                        }
                        CodecKind::Hevc => {
                            if let Some(ref vps_data) = cache.hevc_vps {
                                prepend.push(EncodedPacket {
                                    data: vps_data.clone(),
                                    pts: first_pts,
                                    dts: first_pts,
                                    stream: StreamType::Video,
                                    is_keyframe: false,
                                    resolution: None,
                                });
                            }
                            if let Some(ref sps_data) = cache.hevc_sps {
                                prepend.push(EncodedPacket {
                                    data: sps_data.clone(),
                                    pts: first_pts,
                                    dts: first_pts,
                                    stream: StreamType::Video,
                                    is_keyframe: false,
                                    resolution: None,
                                });
                            }
                            if let Some(ref pps_data) = cache.hevc_pps {
                                prepend.push(EncodedPacket {
                                    data: pps_data.clone(),
                                    pts: first_pts,
                                    dts: first_pts,
                                    stream: StreamType::Video,
                                    is_keyframe: false,
                                    resolution: None,
                                });
                            }
                        }
                    }

                    if !prepend.is_empty() {
                        trace!(
                            "snapshot_from: prepending {} parameter sets before keyframe at idx {}",
                            prepend.len(),
                            start_idx
                        );
                        let mut final_result = Vec::with_capacity(prepend.len() + result.len());
                        final_result.extend(prepend);
                        final_result.extend(result);
                        return Ok(TrackedSnapshot::new(final_result, Arc::clone(inner)));
                    }
                }
            }
        }

        Ok(TrackedSnapshot::new(result, Arc::clone(inner)))
    }

    /// Clears all packets from the buffer.
    ///
    /// Resets the write index and clears all packet slots.
    /// Parameter set cache is preserved for efficient reuse.
    ///
    /// # Thread Safety
    ///
    /// Should be called when no producer is actively pushing packets.
    pub fn clear(&self) {
        let inner = &self.inner;

        // Log state before clear
        let bytes_before = inner.total_bytes.load(Ordering::Relaxed);
        let packets_before = inner
            .write_idx
            .load(Ordering::Relaxed)
            .saturating_sub(inner.evict_frontier.load(Ordering::Relaxed));

        debug!(
            "Buffer clear START: {} packets, {:.1}MB",
            packets_before,
            bytes_before as f64 / 1_048_576.0
        );

        for i in 0..inner.capacity {
            let slot = &inner.slots[i];
            // Use blocking lock with poison recovery — try_lock could silently
            // skip slots if a snapshot consumer holds the lock, leaking packets.
            let mut packet_guard = slot.packet.lock();
            *packet_guard = None;
        }

        inner.write_idx.store(0, Ordering::Release);
        inner.evict_frontier.store(0, Ordering::Release);
        inner.total_bytes.store(0, Ordering::Release);
        inner.keyframe_count.store(0, Ordering::Release);
        inner.newest_pts.store(0, Ordering::Release);
        inner.param_cache_complete.store(false, Ordering::Release);
        inner
            .param_cache_pushes_since_complete
            .store(0, Ordering::Release);
        *inner
            .first_video_info
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;

        let cache = inner.param_cache.lock().unwrap_or_else(|e| e.into_inner());
        debug!(
            "Buffer clear DONE: cleared {} packets ({:.1}MB), param cache preserved (H.264 SPS: {}, PPS: {} | HEVC VPS: {}, SPS: {}, PPS: {})",
            packets_before,
            bytes_before as f64 / 1_048_576.0,
            cache.h264_sps.is_some(),
            cache.h264_pps.is_some(),
            cache.hevc_vps.is_some(),
            cache.hevc_sps.is_some(),
            cache.hevc_pps.is_some()
        );
    }

    /// Completely resets the buffer and parameter caches.
    ///
    /// This differs from `clear()` in that it also clears the cached
    /// codec parameter sets (SPS/PPS/VPS) so the buffer is effectively
    /// restarted from a clean state. Useful when the user explicitly
    /// requests a full replay reset (e.g. after saving a clip and wanting
    /// a fresh buffer).
    pub fn restart(&self) {
        let inner = &self.inner;

        // Log state before restart
        let bytes_before = inner.total_bytes.load(Ordering::Relaxed);
        let packets_before = inner
            .write_idx
            .load(Ordering::Relaxed)
            .saturating_sub(inner.evict_frontier.load(Ordering::Relaxed));

        debug!(
            "Buffer restart START: {} packets, {:.1}MB",
            packets_before,
            bytes_before as f64 / 1_048_576.0
        );

        // Clear all slots (blocking locks to ensure no packet remains)
        for i in 0..inner.capacity {
            let slot = &inner.slots[i];
            let mut packet_guard = slot.packet.lock();
            *packet_guard = None;
        }

        // Reset indexes and stats
        inner.write_idx.store(0, Ordering::Release);
        inner.evict_frontier.store(0, Ordering::Release);
        inner.total_bytes.store(0, Ordering::Release);
        inner.keyframe_count.store(0, Ordering::Release);
        inner.newest_pts.store(0, Ordering::Release);
        inner.param_cache_complete.store(false, Ordering::Release);
        inner
            .param_cache_pushes_since_complete
            .store(0, Ordering::Release);
        *inner
            .first_video_info
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;

        // Clear cached parameter sets as part of a full restart
        {
            let mut cache = inner.param_cache.lock().unwrap_or_else(|e| e.into_inner());
            cache.h264_sps = None;
            cache.h264_pps = None;
            cache.hevc_vps = None;
            cache.hevc_sps = None;
            cache.hevc_pps = None;
            cache.codec_kind = CodecKind::H264;
        }

        debug!(
            "Buffer restart DONE: cleared {} packets ({:.1}MB), parameter cache cleared",
            packets_before,
            bytes_before as f64 / 1_048_576.0
        );
    }

    /// Clears the parameter set cache and resets tracking state.
    ///
    /// Called periodically (every 1000 pushes after cache completion) to prevent
    /// stale parameter sets and to handle encoder reconfiguration. Also clears
    /// first_video_info so the next video packet becomes the new reference point.
    fn clear_parameter_cache(&self) {
        let inner = &self.inner;

        debug!("Clearing parameter cache (periodic refresh)");

        // Clear the cached parameter data
        {
            let mut cache = inner.param_cache.lock().unwrap_or_else(|e| e.into_inner());
            cache.h264_sps = None;
            cache.h264_pps = None;
            cache.hevc_vps = None;
            cache.hevc_sps = None;
            cache.hevc_pps = None;
        }

        // Reset completion state and push counter
        inner.param_cache_complete.store(false, Ordering::Release);
        inner
            .param_cache_pushes_since_complete
            .store(0, Ordering::Release);

        // Clear first_video_info so next video packet becomes new reference
        *inner
            .first_video_info
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// Gets current buffer statistics.
    ///
    /// # Returns
    ///
    /// BufferStats containing:
    /// - `duration_secs`: Duration of buffered content
    /// - `total_bytes`: Total memory usage
    /// - `packet_count`: Number of packets in buffer
    /// - `keyframe_count`: Number of keyframes
    /// - `memory_usage_percent`: Percentage of max memory used
    #[must_use]
    pub fn stats(&self) -> BufferStats {
        let inner = &self.inner;
        let write_idx = inner.write_idx.load(Ordering::Acquire);
        // Acquire ordering ensures we see latest counter values from producer thread
        let total_bytes = inner.total_bytes.load(Ordering::Acquire);
        let keyframe_count = inner.keyframe_count.load(Ordering::Acquire);
        let evict_frontier = inner.evict_frontier.load(Ordering::Acquire);
        let actual_start = write_idx.saturating_sub(inner.capacity).max(evict_frontier);

        let memory_usage_percent = if inner.max_memory_bytes > 0 {
            (total_bytes as f32 / inner.max_memory_bytes as f32) * 100.0
        } else {
            0.0
        };

        // Detailed stats logging for debugging
        if write_idx % 300 == 0 && write_idx > 0 {
            debug!(
                "Buffer stats: write_idx={}, evict_frontier={}, capacity={}, packets={}, keyframes={}, bytes={:.1}MB/{:.1}MB ({:.0}%)",
                write_idx,
                evict_frontier,
                inner.capacity,
                write_idx.saturating_sub(actual_start),
                keyframe_count,
                total_bytes as f64 / 1_048_576.0,
                inner.max_memory_bytes as f64 / 1_048_576.0,
                memory_usage_percent
            );
        }

        let duration_secs = if write_idx >= 2 {
            // Read actual oldest packet's PTS from its slot (evict_frontier aware).
            let oldest_slot = &inner.slots[actual_start & inner.mask];
            let oldest_pts = if let Some(g) = oldest_slot.packet.try_lock() {
                g.as_ref().map_or(0, |p| p.pts)
            } else {
                0
            };
            let newest = inner.newest_pts.load(Ordering::Relaxed);
            let qpc_freq = qpc_frequency() as f64;
            if newest > oldest_pts && qpc_freq > 0.0 {
                (newest - oldest_pts) as f64 / qpc_freq
            } else {
                0.0
            }
        } else {
            0.0
        };

        let packet_count = write_idx.saturating_sub(actual_start);

        BufferStats {
            duration_secs,
            total_bytes,
            packet_count,
            keyframe_count,
            memory_usage_percent: memory_usage_percent.min(100.0),
        }
    }

    /// Returns the number of bytes currently pinned by in-flight snapshots.
    ///
    /// This is memory that has been cloned from the ring but is still being
    /// processed (e.g., being encoded to disk). The ring's `total_bytes` doesn't
    /// account for these pinned allocations, so this method provides visibility
    /// into the actual RSS beyond the ring's configured budget.
    ///
    /// **Note:** This uses `Bytes::len()` (logical slice length), not the backing
    /// allocation size. If packets are views into larger `BytesMut` pages, this
    /// will underreport actual RSS — it's a lower bound, not exact.
    #[must_use]
    pub fn pinned_bytes(&self) -> usize {
        self.inner
            .outstanding_snapshot_bytes
            .load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn oldest_pts(&self) -> Option<i64> {
        let inner = &self.inner;
        let write_idx = inner.write_idx.load(Ordering::Acquire);
        if write_idx == 0 {
            return None;
        }

        let oldest_idx = write_idx.saturating_sub(inner.capacity);
        let evict_frontier = inner.evict_frontier.load(Ordering::Acquire);
        let oldest_idx = oldest_idx.max(evict_frontier);
        let slot_idx = oldest_idx & inner.mask;
        let slot = &inner.slots[slot_idx];

        if let Some(packet_guard) = slot.packet.try_lock() {
            packet_guard.as_ref().map(|p| p.pts)
        } else {
            None
        }
    }

    #[must_use]
    pub fn newest_pts(&self) -> Option<i64> {
        let inner = &self.inner;
        let write_idx = inner.write_idx.load(Ordering::Acquire);
        if write_idx == 0 {
            return None;
        }

        let newest_idx = write_idx - 1;
        let slot_idx = newest_idx & inner.mask;
        let slot = &inner.slots[slot_idx];

        if let Some(packet_guard) = slot.packet.try_lock() {
            packet_guard.as_ref().map(|p| p.pts)
        } else {
            None
        }
    }

    #[must_use]
    pub fn first_packet_resolution(&self) -> Option<(u32, u32)> {
        let inner = &self.inner;
        let write_idx = inner.write_idx.load(Ordering::Acquire);
        if write_idx == 0 {
            return None;
        }

        let start_idx = write_idx.saturating_sub(inner.capacity);
        let evict_frontier = inner.evict_frontier.load(Ordering::Acquire);
        let start_idx = start_idx.max(evict_frontier);
        let slot_idx = start_idx & inner.mask;
        let slot = &inner.slots[slot_idx];

        if let Some(packet_guard) = slot.packet.try_lock() {
            packet_guard.as_ref().and_then(|p| p.resolution)
        } else {
            None
        }
    }

    #[must_use]
    pub fn has_keyframe(&self) -> bool {
        self.inner.keyframe_count.load(Ordering::Relaxed) > 0
    }
}
