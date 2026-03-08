//! Auto-generated module
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use std::sync::OnceLock;

/// Cached QPC frequency (queried once, reused everywhere)
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

pub(crate) fn h264_nal_type(data: &[u8]) -> Option<u8> {
    if data.len() >= 5 && data[0..4] == [0x00, 0x00, 0x00, 0x01] {
        return Some(data[4] & 0x1f);
    }
    if data.len() >= 4 && data[0..3] == [0x00, 0x00, 0x01] {
        return Some(data[3] & 0x1f);
    }
    None
}

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
    use crate::buffer::ring::lockfree::LockFreeReplayBuffer;
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
    fn test_soft_clear_clears_all_state() {
        let buffer = LockFreeReplayBuffer::new(&make_config(120, 512)).unwrap();

        for i in 0..10 {
            let is_keyframe = i % 3 == 0;
            buffer.push(create_test_packet(i * 1_000_000, is_keyframe, 1024));
        }

        let stats_before = buffer.stats();
        assert_eq!(stats_before.packet_count, 10);
        assert!(
            stats_before.keyframe_count > 0,
            "Should have keyframes before soft clear"
        );

        // Soft clear should clear all state including keyframe index
        buffer.soft_clear();

        let stats_after = buffer.stats();
        assert_eq!(stats_after.packet_count, 0, "Packets should be cleared");
        assert_eq!(stats_after.total_bytes, 0, "Bytes should be cleared");
        assert_eq!(
            stats_after.keyframe_count, 0,
            "Keyframe index should be cleared to prevent stale indices"
        );

        // Add more packets and verify keyframe tracking works from fresh state
        for i in 10..20 {
            let is_keyframe = i % 3 == 0;
            buffer.push(create_test_packet(i * 1_000_000, is_keyframe, 1024));
        }

        let stats_final = buffer.stats();
        assert!(
            stats_final.keyframe_count > 0,
            "Should have keyframes after adding new packets"
        );
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

        let qpc = qpc_frequency() as i64;
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

        buffer.soft_clear();

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
    fn test_soft_clear_preserves_hevc_parameter_sets() {
        let buffer = LockFreeReplayBuffer::new(&make_config(120, 512)).unwrap();

        buffer.push(create_hevc_vps_packet(0));
        buffer.push(create_hevc_sps_packet(1_000_000));
        buffer.push(create_hevc_pps_packet(2_000_000));

        buffer.soft_clear();

        assert_eq!(buffer.stats().packet_count, 0);
        assert_eq!(buffer.stats().keyframe_count, 0);

        buffer.push(create_hevc_idr_packet(4_000_000));
        let snapshot = buffer.snapshot().unwrap();
        assert_eq!(snapshot.len(), 4);
        assert_eq!(super::hevc_nal_type(&snapshot[0].data), Some(32));
        assert_eq!(super::hevc_nal_type(&snapshot[1].data), Some(33));
        assert_eq!(super::hevc_nal_type(&snapshot[2].data), Some(34));
    }
}
