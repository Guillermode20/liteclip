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
//! use liteclip_replay::buffer::ring::LockFreeReplayBuffer;
//! use liteclip_replay::config::Config;
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
use tracing::{debug, info, trace};

use super::functions::{h264_nal_type, hevc_nal_type, qpc_frequency};
use super::types::BufferStats;

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
    pub fn is_parameter_set(&self) -> bool {
        matches!(self, FirstVideoKind::H264Sps | FirstVideoKind::HevcVps)
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
    write_idx: AtomicUsize,
    evict_frontier: AtomicUsize,
    max_memory_bytes: usize,
    total_bytes: AtomicUsize,
    keyframe_count: AtomicUsize,
    newest_pts: AtomicI64,
    param_cache: std::sync::Mutex<ParameterCache>,
    param_cache_complete: std::sync::atomic::AtomicBool,
    first_video_info: std::sync::Mutex<Option<(usize, FirstVideoKind)>>,
}

#[repr(align(64))]
struct Slot {
    packet: std::sync::Mutex<Option<EncodedPacket>>,
}

impl Slot {
    fn new() -> Self {
        Self {
            packet: std::sync::Mutex::new(None),
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
    /// # Returns
    ///
    /// A new LockFreeReplayBuffer instance.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use liteclip_replay::buffer::ring::LockFreeReplayBuffer;
    /// use liteclip_replay::config::Config;
    ///
    /// let config = Config::default();
    /// let buffer = LockFreeReplayBuffer::new(&config).unwrap();
    /// ```
    pub fn new(config: &crate::config::Config) -> BufferResult<Self> {
        let duration = Duration::from_secs(config.general.replay_duration_secs as u64 + 1);
        let effective_memory_limit_mb = config.effective_replay_memory_limit_mb();
        let max_memory_bytes = (effective_memory_limit_mb as usize).saturating_mul(1024 * 1024);

        let video_packets_per_sec = config.video.framerate as f32;
        let audio_streams =
            (config.audio.capture_system as u8 + config.audio.capture_mic as u8) as f32;
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
                write_idx: AtomicUsize::new(0),
                evict_frontier: AtomicUsize::new(0),
                max_memory_bytes,
                total_bytes: AtomicUsize::new(0),
                keyframe_count: AtomicUsize::new(0),
                newest_pts: AtomicI64::new(0),
                param_cache: std::sync::Mutex::new(ParameterCache::default()),
                param_cache_complete: std::sync::atomic::AtomicBool::new(false),
                first_video_info: std::sync::Mutex::new(None),
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

        self.cache_parameter_sets(&packet);

        if matches!(packet.stream, StreamType::Video) {
            let mut first_video_info = inner.first_video_info.lock().unwrap();
            if first_video_info.is_none() {
                let kind = self.detect_first_video_kind(packet.data.as_ref());
                *first_video_info = Some((inner.write_idx.load(Ordering::Relaxed), kind));
            }
        }

        let write_idx = inner.write_idx.fetch_add(1, Ordering::Release);
        let slot_idx = write_idx & inner.mask;
        let slot = &inner.slots[slot_idx];

        let old_packet = {
            let mut packet_guard = slot.packet.lock().unwrap();
            let old = packet_guard.take();
            *packet_guard = Some(packet);
            old
        };

        if let Some(ref old) = old_packet {
            inner
                .total_bytes
                .fetch_sub(old.data.len(), Ordering::Relaxed);
            if old.is_keyframe {
                inner.keyframe_count.fetch_sub(1, Ordering::Relaxed);
            }
        }

        inner.total_bytes.fetch_add(packet_size, Ordering::Relaxed);
        if is_keyframe {
            inner.keyframe_count.fetch_add(1, Ordering::Relaxed);
        }

        inner.newest_pts.store(packet_pts, Ordering::Release);

        // Enforce memory budget: if total_bytes exceeds max_memory_bytes, proactively
        // evict the oldest packets (those the ring would discard next anyway) until we
        // are back under the limit.  This prevents the initial fill phase from growing
        // RAM unboundedly when the configured bitrate generates packets larger than the
        // packet-count estimate assumed when sizing the ring capacity.
        if inner.max_memory_bytes > 0 {
            while inner.total_bytes.load(Ordering::Relaxed) > inner.max_memory_bytes {
                let evict = inner.evict_frontier.load(Ordering::Relaxed);
                // Never evict the packet we just wrote (at index `write_idx`).
                if evict >= write_idx {
                    break;
                }
                let evict_slot_idx = evict & inner.mask;
                let slot = &inner.slots[evict_slot_idx];
                {
                    let mut guard = slot.packet.lock().unwrap();
                    if let Some(old) = guard.take() {
                        inner
                            .total_bytes
                            .fetch_sub(old.data.len(), Ordering::Relaxed);
                        if old.is_keyframe {
                            inner.keyframe_count.fetch_sub(1, Ordering::Relaxed);
                        }
                    }
                }
                inner.evict_frontier.fetch_add(1, Ordering::Release);
            }
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

        let mut cache = inner.param_cache.lock().unwrap();

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
    /// # Returns
    ///
    /// Vector of all encoded packets in chronological order.
    ///
    /// # Thread Safety
    ///
    /// Safe to call from multiple consumer threads.
    pub fn snapshot(&self) -> BufferResult<Vec<EncodedPacket>> {
        let inner = &self.inner;
        let write_idx = inner.write_idx.load(Ordering::Acquire);

        if write_idx == 0 {
            return Ok(vec![]);
        }

        let capacity = inner.capacity;
        let evict_frontier = inner.evict_frontier.load(Ordering::Acquire);
        let start_idx = write_idx.saturating_sub(capacity).max(evict_frontier);
        let count = write_idx - start_idx;

        let mut result = Vec::with_capacity(count);

        for i in start_idx..write_idx {
            let slot_idx = i & inner.mask;
            let slot = &inner.slots[slot_idx];

            if let Ok(packet_guard) = slot.packet.try_lock() {
                if let Some(ref packet) = *packet_guard {
                    result.push(packet.clone());
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

            let cache = inner.param_cache.lock().unwrap();
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
            return Ok(final_result);
        }

        Ok(result)
    }

    /// Gets a snapshot starting from a specific PTS.
    ///
    /// Finds the nearest keyframe at or after the given PTS and returns
    /// all packets from that keyframe onward.
    ///
    /// # Arguments
    ///
    /// * `start_pts` - The starting presentation timestamp (in stream timebase).
    ///
    /// # Returns
    ///
    /// Vector of encoded packets starting from a keyframe.
    ///
    /// # Thread Safety
    ///
    /// Safe to call from multiple consumer threads.
    pub fn snapshot_from(&self, start_pts: i64) -> BufferResult<Vec<EncodedPacket>> {
        let inner = &self.inner;
        let write_idx = inner.write_idx.load(Ordering::Acquire);

        if write_idx == 0 {
            return Ok(vec![]);
        }

        let first_idx = write_idx.saturating_sub(inner.capacity);
        let evict_frontier = inner.evict_frontier.load(Ordering::Acquire);
        let first_idx = first_idx.max(evict_frontier);

        let capacity_estimate = write_idx.saturating_sub(first_idx);
        let mut packets: Vec<(usize, EncodedPacket)> = Vec::with_capacity(capacity_estimate);
        let mut first_keyframe_at_or_after = None;
        let mut last_keyframe_at_or_before = None;

        for i in first_idx..write_idx {
            let slot_idx = i & inner.mask;
            let slot = &inner.slots[slot_idx];

            if let Ok(packet_guard) = slot.packet.try_lock() {
                if let Some(ref packet) = *packet_guard {
                    if packet.is_keyframe {
                        if packet.pts >= start_pts && first_keyframe_at_or_after.is_none() {
                            first_keyframe_at_or_after = Some(i);
                        }
                        if packet.pts <= start_pts {
                            last_keyframe_at_or_before = Some(i);
                        }
                    }
                    packets.push((i, packet.clone()));
                }
            }
        }

        let start_idx = first_keyframe_at_or_after
            .or(last_keyframe_at_or_before)
            .unwrap_or(first_idx);

        let video_count = packets
            .iter()
            .filter(|(_, p)| matches!(p.stream, StreamType::Video))
            .count();
        let keyframe_count = packets.iter().filter(|(_, p)| p.is_keyframe).count();
        debug!(
            "snapshot_from: all_packets={} ({} video, {} keyframes), start_pts={}, start_idx={}",
            packets.len(),
            video_count,
            keyframe_count,
            start_pts,
            start_idx
        );

        let drain_start = packets
            .iter()
            .position(|(i, _)| *i >= start_idx)
            .unwrap_or(packets.len());

        let result: Vec<EncodedPacket> = packets.drain(drain_start..).map(|(_, p)| p).collect();

        let result_video = result
            .iter()
            .filter(|p| matches!(p.stream, StreamType::Video))
            .count();
        debug!(
            "snapshot_from: result={} packets ({} video)",
            result.len(),
            result_video
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
                    let cache = self.inner.param_cache.lock().unwrap();
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
                        return Ok(final_result);
                    }
                }
            }
        }

        Ok(result)
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

        for i in 0..inner.capacity {
            let slot = &inner.slots[i];
            if let Ok(mut packet_guard) = slot.packet.try_lock() {
                *packet_guard = None;
            }
        }

        inner.write_idx.store(0, Ordering::Release);
        inner.evict_frontier.store(0, Ordering::Release);
        inner.total_bytes.store(0, Ordering::Release);
        inner.keyframe_count.store(0, Ordering::Release);
        inner.param_cache_complete.store(false, Ordering::Release);
        *inner.first_video_info.lock().unwrap() = None;

        let cache = inner.param_cache.lock().unwrap();
        debug!(
            "Lock-free buffer cleared (H.264 SPS: {}, PPS: {} | HEVC VPS: {}, SPS: {}, PPS: {})",
            cache.h264_sps.is_some(),
            cache.h264_pps.is_some(),
            cache.hevc_vps.is_some(),
            cache.hevc_sps.is_some(),
            cache.hevc_pps.is_some()
        );
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
    pub fn stats(&self) -> BufferStats {
        let inner = &self.inner;
        let write_idx = inner.write_idx.load(Ordering::Acquire);
        let total_bytes = inner.total_bytes.load(Ordering::Relaxed);
        let keyframe_count = inner.keyframe_count.load(Ordering::Relaxed);
        let evict_frontier = inner.evict_frontier.load(Ordering::Acquire);
        let actual_start = write_idx.saturating_sub(inner.capacity).max(evict_frontier);

        let memory_usage_percent = if inner.max_memory_bytes > 0 {
            (total_bytes as f32 / inner.max_memory_bytes as f32) * 100.0
        } else {
            0.0
        };

        let duration_secs = if write_idx >= 2 {
            // Read actual oldest packet's PTS from its slot (evict_frontier aware).
            let oldest_slot = &inner.slots[actual_start & inner.mask];
            let oldest_pts = if let Ok(g) = oldest_slot.packet.try_lock() {
                g.as_ref().map(|p| p.pts).unwrap_or(0)
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

        if let Ok(packet_guard) = slot.packet.try_lock() {
            packet_guard.as_ref().map(|p| p.pts)
        } else {
            None
        }
    }

    pub fn newest_pts(&self) -> Option<i64> {
        let inner = &self.inner;
        let write_idx = inner.write_idx.load(Ordering::Acquire);
        if write_idx == 0 {
            return None;
        }

        let newest_idx = write_idx - 1;
        let slot_idx = newest_idx & inner.mask;
        let slot = &inner.slots[slot_idx];

        if let Ok(packet_guard) = slot.packet.try_lock() {
            packet_guard.as_ref().map(|p| p.pts)
        } else {
            None
        }
    }

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

        if let Ok(packet_guard) = slot.packet.try_lock() {
            packet_guard.as_ref().and_then(|p| p.resolution)
        } else {
            None
        }
    }

    pub fn has_keyframe(&self) -> bool {
        self.inner.keyframe_count.load(Ordering::Relaxed) > 0
    }
}
