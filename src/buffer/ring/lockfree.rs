//! Lock-free replay buffer implementation
//!
//! Uses a ring buffer with atomic indices for single-producer, multi-consumer access.
//! The producer writes atomically; consumers read via optimistic locking (seqlock pattern).

use crate::encode::{EncodedPacket, StreamType};
use anyhow::Result;
use bytes::Bytes;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, trace};

use super::functions::{h264_nal_type, hevc_nal_type, qpc_frequency};
use super::types::BufferStats;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodecKind {
    #[default]
    H264,
    Hevc,
}

struct ParameterCache {
    codec_kind: CodecKind,
    h264_sps: Option<Bytes>,
    h264_pps: Option<Bytes>,
    hevc_vps: Option<Bytes>,
    hevc_sps: Option<Bytes>,
    hevc_pps: Option<Bytes>,
}

impl Default for ParameterCache {
    fn default() -> Self {
        Self {
            codec_kind: CodecKind::default(),
            h264_sps: None,
            h264_pps: None,
            hevc_vps: None,
            hevc_sps: None,
            hevc_pps: None,
        }
    }
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

#[derive(Clone)]
pub struct LockFreeReplayBuffer {
    inner: Arc<LockFreeInner>,
}

struct LockFreeInner {
    slots: Box<[Slot]>,
    capacity: usize,
    mask: usize,
    write_idx: AtomicUsize,
    max_memory_bytes: usize,
    total_bytes: AtomicUsize,
    keyframe_count: AtomicUsize,
    oldest_pts: AtomicI64,
    newest_pts: AtomicI64,
    param_cache: std::sync::Mutex<ParameterCache>,
    first_video_info: std::sync::Mutex<Option<(usize, FirstVideoKind)>>,
}

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
    pub fn new(config: &crate::config::Config) -> Result<Self> {
        let duration = Duration::from_secs(config.general.replay_duration_secs as u64 + 1);
        let effective_memory_limit_mb = config.effective_replay_memory_limit_mb();
        let max_memory_bytes = (effective_memory_limit_mb as usize).saturating_mul(1024 * 1024);

        let estimated_packets = (duration.as_secs_f32() * 60.0).max(100.0) as usize;
        let capacity = estimated_packets.next_power_of_two();
        let mask = capacity - 1;

        let mut slots = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            slots.push(Slot::new());
        }

        debug!(
            "Creating LockFreeReplayBuffer: {} seconds, {} MB max, {} slots",
            duration.as_secs(),
            effective_memory_limit_mb,
            capacity
        );

        Ok(Self {
            inner: Arc::new(LockFreeInner {
                slots: slots.into_boxed_slice(),
                capacity,
                mask,
                write_idx: AtomicUsize::new(0),
                max_memory_bytes,
                total_bytes: AtomicUsize::new(0),
                keyframe_count: AtomicUsize::new(0),
                oldest_pts: AtomicI64::new(0),
                newest_pts: AtomicI64::new(0),
                param_cache: std::sync::Mutex::new(ParameterCache::default()),
                first_video_info: std::sync::Mutex::new(None),
            }),
        })
    }

    pub fn push_batch(&self, packets: impl IntoIterator<Item = EncodedPacket>) {
        for packet in packets {
            self.push_single(packet);
        }
    }

    pub fn push(&self, packet: EncodedPacket) {
        self.push_single(packet);
    }

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
    }

    fn cache_parameter_sets(&self, packet: &EncodedPacket) {
        if !matches!(packet.stream, StreamType::Video) || packet.data.is_empty() {
            return;
        }

        let inner = &self.inner;
        let data = packet.data.as_ref();

        let mut cache = inner.param_cache.lock().unwrap();

        let mut nals: Vec<(usize, usize, u8, u8)> = Vec::new();
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

            nals.push((i, start_code_len, hevc_nal, h264_nal));
            i = nal_start + 1;
        }

        for (idx, (start, sc_len, hevc_nal, h264_nal)) in nals.iter().enumerate() {
            let _nal_start = start + sc_len;

            let next_start = nals
                .get(idx + 1)
                .map(|(s, _, _, _)| *s)
                .unwrap_or(data.len());

            let nal_data = Bytes::copy_from_slice(&data[*start..next_start]);

            let already_hevc =
                cache.hevc_vps.is_some() || cache.hevc_sps.is_some() || cache.hevc_pps.is_some();

            match hevc_nal {
                32 => {
                    cache.hevc_vps = Some(nal_data);
                    cache.codec_kind = CodecKind::Hevc;
                    trace!("Cached HEVC VPS ({} bytes)", next_start - start);
                }
                33 => {
                    cache.hevc_sps = Some(nal_data);
                    cache.codec_kind = CodecKind::Hevc;
                    trace!("Cached HEVC SPS ({} bytes)", next_start - start);
                }
                34 => {
                    cache.hevc_pps = Some(nal_data);
                    cache.codec_kind = CodecKind::Hevc;
                    trace!("Cached HEVC PPS ({} bytes)", next_start - start);
                }
                _ => {
                    if !already_hevc {
                        match h264_nal {
                            7 => {
                                cache.h264_sps = Some(nal_data);
                                cache.codec_kind = CodecKind::H264;
                                trace!("Cached H.264 SPS ({} bytes)", next_start - start);
                            }
                            8 => {
                                cache.h264_pps = Some(nal_data);
                                cache.codec_kind = CodecKind::H264;
                                trace!("Cached H.264 PPS ({} bytes)", next_start - start);
                            }
                            _ => {}
                        }
                    }
                }
            }
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

    pub fn snapshot(&self) -> Result<Vec<EncodedPacket>> {
        let inner = &self.inner;
        let write_idx = inner.write_idx.load(Ordering::Acquire);

        if write_idx == 0 {
            return Ok(vec![]);
        }

        let capacity = inner.capacity;
        let start_idx = write_idx.saturating_sub(capacity);
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

    pub fn snapshot_from(&self, start_pts: i64) -> Result<Vec<EncodedPacket>> {
        let all_packets = self.snapshot()?;

        if all_packets.is_empty() {
            return Ok(vec![]);
        }

        let video_count = all_packets
            .iter()
            .filter(|p| matches!(p.stream, StreamType::Video))
            .count();
        let audio_count = all_packets.len() - video_count;
        let keyframe_count = all_packets.iter().filter(|p| p.is_keyframe).count();
        debug!(
            "snapshot_from: all_packets={} ({} video, {} audio, {} keyframes), start_pts={}",
            all_packets.len(),
            video_count,
            audio_count,
            keyframe_count,
            start_pts
        );

        let keyframe_idx = all_packets
            .iter()
            .position(|p| p.is_keyframe && p.pts >= start_pts)
            .or_else(|| {
                all_packets
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, p)| p.is_keyframe && p.pts <= start_pts)
                    .map(|(i, _)| i)
            });

        let start_idx = keyframe_idx.unwrap_or(0);

        if start_idx > 0 {
            debug!(
                "snapshot_from: skipping {} packets to reach keyframe at idx {}",
                start_idx, start_idx
            );
        }

        let result: Vec<EncodedPacket> = all_packets.into_iter().skip(start_idx).collect();

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

    pub fn clear(&self) {
        let inner = &self.inner;

        for i in 0..inner.capacity {
            let slot = &inner.slots[i];
            if let Ok(mut packet_guard) = slot.packet.try_lock() {
                *packet_guard = None;
            }
        }

        inner.write_idx.store(0, Ordering::Release);
        inner.total_bytes.store(0, Ordering::Release);
        inner.keyframe_count.store(0, Ordering::Release);
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

    pub fn soft_clear(&self) {
        self.clear();
    }

    pub fn stats(&self) -> BufferStats {
        let inner = &self.inner;
        let write_idx = inner.write_idx.load(Ordering::Acquire);
        let total_bytes = inner.total_bytes.load(Ordering::Relaxed);
        let keyframe_count = inner.keyframe_count.load(Ordering::Relaxed);

        let memory_usage_percent = if inner.max_memory_bytes > 0 {
            (total_bytes as f32 / inner.max_memory_bytes as f32) * 100.0
        } else {
            0.0
        };

        let duration_secs = if write_idx >= 2 {
            let oldest = inner.oldest_pts.load(Ordering::Relaxed);
            let newest = inner.newest_pts.load(Ordering::Relaxed);
            let qpc_freq = qpc_frequency() as f64;
            ((newest - oldest) as f64) / qpc_freq
        } else {
            0.0
        };

        let packet_count = write_idx.min(inner.capacity);

        BufferStats {
            duration_secs,
            total_bytes,
            packet_count,
            keyframe_count,
            memory_usage_percent: memory_usage_percent.min(100.0),
        }
    }

    pub fn is_full(&self) -> bool {
        let inner = &self.inner;
        inner.write_idx.load(Ordering::Relaxed) >= inner.capacity
    }

    pub fn oldest_pts(&self) -> Option<i64> {
        let inner = &self.inner;
        let write_idx = inner.write_idx.load(Ordering::Acquire);
        if write_idx == 0 {
            return None;
        }

        let oldest_idx = write_idx.saturating_sub(inner.capacity);
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
