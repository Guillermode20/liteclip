//! Helper utilities for the lock-free ring buffer.

use std::sync::OnceLock;

/// Returns the Windows QueryPerformanceCounter (QPC) frequency.
///
/// This frequency is used to convert high-resolution hardware timestamps
/// into seconds or standard media timebases. The value is cached after
/// the first query to avoid redundant syscalls.
pub fn qpc_frequency() -> i64 {
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

/// Extracts the NAL unit type from an H.264 byte stream.
///
/// This function identifies the NAL type by looking for the `00 00 00 01` or
/// `00 00 01` start code and masking the following byte with `0x1f`.
///
/// Useful for Identifying:
/// - IDR Frames (Keyframes: 5)
/// - SPS (Sequence Parameter Set: 7)
/// - PPS (Picture Parameter Set: 8)
pub(crate) fn h264_nal_type(data: &[u8]) -> Option<u8> {
    if data.len() >= 5 && data[0..4] == [0x00, 0x00, 0x00, 0x01] {
        return Some(data[4] & 0x1f);
    }
    if data.len() >= 4 && data[0..3] == [0x00, 0x00, 0x01] {
        return Some(data[3] & 0x1f);
    }
    None
}

/// Extracts the NAL unit type from an HEVC (H.265) byte stream.
///
/// This function identifies the NAL type by looking for the `00 00 00 01` or
/// `00 00 01` start code, extracting the NAL unit header (typically 2 bytes),
/// and shifting the first byte right by 1 and masking with `0x3f`.
///
/// Useful for Identifying:
/// - IDR Frames (Keyframes: 19 or 20)
/// - VPS (Video Parameter Set: 32)
/// - SPS (Sequence Parameter Set: 33)
/// - PPS (Picture Parameter Set: 34)
pub(crate) fn hevc_nal_type(data: &[u8]) -> Option<u8> {
    if data.len() >= 6 && data[0..4] == [0x00, 0x00, 0x00, 0x01] {
        return Some((data[4] >> 1) & 0x3f);
    }
    if data.len() >= 5 && data[0..3] == [0x00, 0x00, 0x01] {
        return Some((data[3] >> 1) & 0x3f);
    }
    None
}

#[cfg(test)]
mod tests {
    use crate::buffer::ring::spmc_ring::LockFreeReplayBuffer;
    use crate::encode::{EncodedPacket, StreamType};
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

    fn make_config(duration_secs: u64, memory_mb: usize) -> crate::config::Config {
        let mut config = crate::config::Config::default();
        config.general.replay_duration_secs = duration_secs as u32;
        config.advanced.memory_limit_mb = memory_mb as u32;
        config
    }

    #[test]
    fn test_buffer_push_and_snapshot() {
        let buffer = LockFreeReplayBuffer::new(&make_config(120, 512)).unwrap();
        for i in 0..10 {
            let packet = create_test_packet(i * 1_000_000, true, 1024);
            buffer.push(packet);
        }
        let snapshot = buffer.snapshot().unwrap();
        assert_eq!(snapshot.len(), 10);
    }

    #[test]
    fn test_memory_budget_enforcement() {
        let buffer = LockFreeReplayBuffer::new(&make_config(120, 1)).unwrap();
        for i in 0..100 {
            let packet = create_test_packet(i * 1_000_000, i % 10 == 0, 50_000);
            buffer.push(packet);
        }
        let stats = buffer.stats();
        assert!(stats.memory_usage_percent <= 100.0);
    }

    #[test]
    fn test_oversized_single_packet_dropped_for_memory_cap() {
        let mut config = make_config(30, 1);
        config.advanced.memory_limit_mb = 1;
        let buffer = LockFreeReplayBuffer::new(&config).unwrap();
        let cap_bytes =
            (config.effective_replay_memory_limit_mb() as usize).saturating_mul(1024 * 1024);
        buffer.push(create_test_packet(0, true, cap_bytes.saturating_mul(2)));
        let stats = buffer.stats();
        assert!(
            stats.total_bytes <= cap_bytes,
            "total_bytes={} expected <= {}",
            stats.total_bytes,
            cap_bytes
        );
    }

    #[test]
    fn test_keyframe_seeking() {
        let buffer = LockFreeReplayBuffer::new(&make_config(120, 512)).unwrap();
        for i in 0..30 {
            let is_keyframe = i % 5 == 0;
            let packet = create_test_packet(i * 1_000_000, is_keyframe, 1024);
            buffer.push(packet);
        }
        let snapshot = buffer.snapshot_from(12_000_000).unwrap();
        assert!(!snapshot.is_empty());
    }

    #[test]
    fn test_snapshot_from_prefers_previous_keyframe() {
        let buffer = LockFreeReplayBuffer::new(&make_config(120, 512)).unwrap();
        for i in 0..15 {
            let is_keyframe = i == 0 || i == 5 || i == 10;
            let packet = create_test_packet(i * 1_000_000, is_keyframe, 1024);
            buffer.push(packet);
        }

        let snapshot = buffer.snapshot_from(7_000_000).unwrap();
        assert!(!snapshot.is_empty());
        assert_eq!(snapshot[0].pts, 5_000_000);
        assert!(snapshot[0].is_keyframe);
    }

    #[test]
    fn test_snapshot_cheap_clone() {
        let buffer = LockFreeReplayBuffer::new(&make_config(120, 512)).unwrap();
        let large_data = vec![0u8; 1_000_000];
        let packet = EncodedPacket {
            data: Bytes::from(large_data),
            pts: 0,
            dts: 0,
            is_keyframe: true,
            stream: StreamType::Video,
            resolution: None,
        };
        buffer.push(packet);
        let snapshot = buffer.snapshot().unwrap();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].data.len(), 1_000_000);
    }

    #[test]
    fn test_clear() {
        let buffer = LockFreeReplayBuffer::new(&make_config(120, 512)).unwrap();
        for i in 0..10 {
            buffer.push(create_test_packet(i * 1_000_000, true, 1024));
        }
        buffer.clear();
        let stats = buffer.stats();
        assert_eq!(stats.packet_count, 0);
        assert_eq!(stats.total_bytes, 0);
        assert_eq!(stats.keyframe_count, 0);
    }

    #[test]
    fn test_eviction_keyframe_index_correctness() {
        let buffer = LockFreeReplayBuffer::new(&make_config(120, 10)).unwrap();
        let mut last_keyframe_pts = 0i64;
        for i in 0..200 {
            let is_kf = i % 10 == 0;
            let pts = i * 1_000_000;
            if is_kf {
                last_keyframe_pts = pts;
            }
            buffer.push(create_test_packet(pts, is_kf, 10_000));
        }
        let snap = buffer.snapshot_from(last_keyframe_pts).unwrap();
        assert!(!snap.is_empty());
    }

    #[test]
    fn test_duration_eviction_keyframe_continuity() {
        use super::qpc_frequency;

        let qpc = qpc_frequency();
        let fps = 30i64;
        let keyframe_every_n_frames = 60i64;
        let buffer = LockFreeReplayBuffer::new(&make_config(5, 1024)).unwrap();

        let step = qpc / fps;
        for i in 0i64..300 {
            let pts = i * step;
            let is_kf = i % keyframe_every_n_frames == 0;
            buffer.push(create_test_packet(pts, is_kf, 1024));

            if i > 150 {
                let kf_count = buffer.stats().keyframe_count;
                assert!(
                    kf_count > 0,
                    "keyframe_count dropped to 0 at frame {} (duration-based eviction bug)",
                    i
                );
            }
        }

        let newest_pts = buffer.newest_pts().unwrap();
        let start_pts = newest_pts - qpc * 5;
        let snap = buffer
            .snapshot_from(start_pts)
            .expect("snapshot_from failed after buffer wrap");
        assert!(!snap.is_empty(), "snapshot_from returned empty after wrap");
        assert!(
            snap.iter().any(|p| p.is_keyframe),
            "snapshot after wrap contains no keyframe packets"
        );
    }

    fn create_hevc_vps_packet(pts: i64) -> EncodedPacket {
        EncodedPacket {
            data: Bytes::from(vec![0x00, 0x00, 0x00, 0x01, 0x40, 0x01, 0x0c, 0x01]),
            pts,
            dts: pts,
            is_keyframe: false,
            stream: StreamType::Video,
            resolution: None,
        }
    }

    fn create_hevc_sps_packet(pts: i64) -> EncodedPacket {
        EncodedPacket {
            data: Bytes::from(vec![0x00, 0x00, 0x00, 0x01, 0x42, 0x01, 0x01, 0x01]),
            pts,
            dts: pts,
            is_keyframe: false,
            stream: StreamType::Video,
            resolution: None,
        }
    }

    fn create_hevc_pps_packet(pts: i64) -> EncodedPacket {
        EncodedPacket {
            data: Bytes::from(vec![0x00, 0x00, 0x00, 0x01, 0x44, 0x01, 0xc1, 0x72]),
            pts,
            dts: pts,
            is_keyframe: false,
            stream: StreamType::Video,
            resolution: None,
        }
    }

    fn create_hevc_idr_packet(pts: i64) -> EncodedPacket {
        EncodedPacket {
            data: Bytes::from(vec![0x00, 0x00, 0x00, 0x01, 0x26, 0x01, 0xaf, 0x1d]),
            pts,
            dts: pts,
            is_keyframe: true,
            stream: StreamType::Video,
            resolution: None,
        }
    }

    #[test]
    fn test_hevc_nal_type_detection() {
        assert_eq!(
            super::hevc_nal_type(&[0x00, 0x00, 0x00, 0x01, 0x40, 0x01]),
            Some(32)
        );
        assert_eq!(
            super::hevc_nal_type(&[0x00, 0x00, 0x00, 0x01, 0x42, 0x01]),
            Some(33)
        );
        assert_eq!(
            super::hevc_nal_type(&[0x00, 0x00, 0x00, 0x01, 0x44, 0x01]),
            Some(34)
        );
        assert_eq!(
            super::hevc_nal_type(&[0x00, 0x00, 0x00, 0x01, 0x26, 0x01]),
            Some(19)
        );
    }

    #[test]
    fn test_hevc_parameter_set_caching() {
        let buffer = LockFreeReplayBuffer::new(&make_config(120, 512)).unwrap();

        buffer.push(create_hevc_vps_packet(0));
        buffer.push(create_hevc_sps_packet(1_000_000));
        buffer.push(create_hevc_pps_packet(2_000_000));
        buffer.push(create_hevc_idr_packet(3_000_000));

        let snapshot = buffer.snapshot().unwrap();
        assert!(snapshot.len() >= 4);
    }

    #[test]
    fn test_hevc_snapshot_prepends_parameter_sets() {
        let buffer = LockFreeReplayBuffer::new(&make_config(120, 512)).unwrap();

        buffer.push(create_hevc_vps_packet(0));
        buffer.push(create_hevc_sps_packet(1_000_000));
        buffer.push(create_hevc_pps_packet(2_000_000));
        buffer.push(create_hevc_idr_packet(3_000_000));

        buffer.clear();

        buffer.push(create_hevc_idr_packet(4_000_000));

        let snapshot = buffer.snapshot().unwrap();
        assert_eq!(snapshot.len(), 4);
        assert_eq!(super::hevc_nal_type(&snapshot[0].data), Some(32));
        assert_eq!(super::hevc_nal_type(&snapshot[1].data), Some(33));
        assert_eq!(super::hevc_nal_type(&snapshot[2].data), Some(34));
        assert_eq!(super::hevc_nal_type(&snapshot[3].data), Some(19));
    }

    #[test]
    fn test_hevc_snapshot_from_prepends_parameter_sets() {
        let buffer = LockFreeReplayBuffer::new(&make_config(120, 512)).unwrap();

        buffer.push(create_hevc_vps_packet(0));
        buffer.push(create_hevc_sps_packet(1_000_000));
        buffer.push(create_hevc_pps_packet(2_000_000));
        buffer.push(create_hevc_idr_packet(3_000_000));

        let snapshot = buffer.snapshot_from(2_500_000).unwrap();
        assert!(!snapshot.is_empty());
        assert!(snapshot.iter().any(|p| p.is_keyframe));
    }

    #[test]
    fn test_clear_preserves_hevc_parameter_sets() {
        let buffer = LockFreeReplayBuffer::new(&make_config(120, 512)).unwrap();

        buffer.push(create_hevc_vps_packet(0));
        buffer.push(create_hevc_sps_packet(1_000_000));
        buffer.push(create_hevc_pps_packet(2_000_000));

        buffer.clear();

        assert_eq!(buffer.stats().packet_count, 0);
        assert_eq!(buffer.stats().keyframe_count, 0);

        buffer.push(create_hevc_idr_packet(4_000_000));
        let snapshot = buffer.snapshot().unwrap();
        assert_eq!(snapshot.len(), 4);
        assert_eq!(super::hevc_nal_type(&snapshot[0].data), Some(32));
        assert_eq!(super::hevc_nal_type(&snapshot[1].data), Some(33));
        assert_eq!(super::hevc_nal_type(&snapshot[2].data), Some(34));
    }

    /// Test that evict_frontier is correctly updated after ring wrap.
    /// This was the root cause of a memory leak: without updating evict_frontier,
    /// memory eviction would evict NEW packets instead of old ones after wrap.
    #[test]
    fn test_evict_frontier_updates_after_ring_wrap() {
        // Create a buffer with small capacity to force ring wrap quickly.
        // Duration=1s at 30fps = ~30 video packets, but we use a tiny memory limit
        // to ensure the ring wraps multiple times.
        let buffer = LockFreeReplayBuffer::new(&make_config(1, 10)).unwrap();

        // Push 500 packets, which should wrap the ring many times.
        // Each packet is 50KB, so 500 packets = 25MB total.
        // With 10MB limit, we should never exceed ~10MB.
        for i in 0..500 {
            let packet = create_test_packet(i * 1_000_000, i % 10 == 0, 50_000);
            buffer.push(packet);
        }

        let stats = buffer.stats();

        // Memory should stay within budget (10MB + some headroom for in-flight eviction)
        assert!(
            stats.memory_usage_percent <= 110.0,
            "Memory usage {}% exceeds budget after ring wrap (expected <=110%)",
            stats.memory_usage_percent
        );

        // The packet count should be bounded, not 500
        assert!(
            stats.packet_count < 500,
            "Packet count {} indicates packets were not evicted",
            stats.packet_count
        );
    }

    /// Test that memory stays bounded during continuous operation with ring wrap.
    /// Simulates long-running recording scenario.
    #[test]
    fn test_memory_stays_bounded_during_long_run() {
        let buffer = LockFreeReplayBuffer::new(&make_config(5, 5)).unwrap();

        // Push 1000 packets over "time", simulating continuous recording.
        // Each packet = 100KB, total = 100MB pushed.
        // With 5MB limit, memory should never exceed ~5-6MB.
        let mut max_memory_percent = 0.0f32;

        for i in 0..1000 {
            let packet = create_test_packet(i * 33_333_333, i % 30 == 0, 100_000);
            buffer.push(packet);

            let stats = buffer.stats();
            max_memory_percent = max_memory_percent.max(stats.memory_usage_percent);
        }

        let final_stats = buffer.stats();

        // Final memory should be within budget
        assert!(
            final_stats.memory_usage_percent <= 110.0,
            "Final memory usage {}% exceeds budget",
            final_stats.memory_usage_percent
        );

        // Max memory during run should also be bounded (allow 10% overshoot for race between add/evict)
        assert!(
            max_memory_percent <= 120.0,
            "Peak memory usage {}% indicates memory leak during ring wrap",
            max_memory_percent
        );
    }
}
