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
#[cfg(test)]
mod tests {
    use crate::buffer::ring::types::ReplayBuffer;
    use crate::encode::{EncodedPacket, StreamType};
    use bytes::Bytes;
    use std::time::Duration;

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
        for i in 0..10 {
            let packet = create_test_packet(i * 1_000_000, true, 1024);
            buffer.push(packet);
        }
        let snapshot = buffer.snapshot().unwrap();
        assert_eq!(snapshot.len(), 10);
    }
    #[test]
    fn test_memory_budget_enforcement() {
        let mut buffer = ReplayBuffer::with_params(Duration::from_secs(120), 1);
        for i in 0..100 {
            let packet = create_test_packet(i * 1_000_000, i % 10 == 0, 50_000);
            buffer.push(packet);
        }
        let stats = buffer.stats();
        assert!(stats.memory_usage_percent <= 100.0);
    }
    #[test]
    fn test_keyframe_seeking() {
        let mut buffer = ReplayBuffer::with_params(Duration::from_secs(120), 512);
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
        let mut buffer = ReplayBuffer::with_params(Duration::from_secs(120), 512);
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
        let mut buffer = ReplayBuffer::with_params(Duration::from_secs(120), 512);
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
    fn test_soft_clear_preserves_keyframes() {
        let mut buffer = ReplayBuffer::with_params(Duration::from_secs(120), 512);

        // Add packets with keyframes
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

        // Soft clear should preserve keyframe tracking
        buffer.soft_clear();

        let stats_after = buffer.stats();
        assert_eq!(stats_after.packet_count, 0, "Packets should be cleared");
        assert_eq!(stats_after.total_bytes, 0, "Bytes should be cleared");
        assert!(
            stats_after.keyframe_count > 0,
            "Keyframes should be preserved after soft clear"
        );

        // Add more packets and verify keyframe tracking still works
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
        let mut buffer = ReplayBuffer::with_params(Duration::from_secs(120), 1);
        let mut last_keyframe_pts = 0i64;
        for i in 0..200 {
            let is_kf = i % 10 == 0;
            let pts = i * 1_000_000;
            if is_kf {
                last_keyframe_pts = pts;
            }
            buffer.push(create_test_packet(pts, is_kf, 10_000));
        }
        let stats = buffer.stats();
        assert!(stats.packet_count < 200);
        let snap = buffer.snapshot_from(last_keyframe_pts).unwrap();
        assert!(!snap.is_empty());
        assert!(snap[0].pts <= last_keyframe_pts + 10_000_000);
    }

    /// Verify keyframe_count never drops to zero during duration-based eviction (the
    /// "buffer wrapped around" failure scenario reported in production).
    ///
    /// Simulates 10 seconds of recording at 30 fps into a 5-second ring buffer, with
    /// an IDR keyframe every 2 seconds (every 60 frames).  After the buffer wraps the
    /// keyframe index must still contain at least one entry so that `save_clip` does
    /// not return "No keyframe available".
    #[test]
    fn test_duration_eviction_keyframe_continuity() {
        use super::qpc_frequency;

        let qpc = qpc_frequency() as i64;
        let fps = 30i64;
        let keyframe_every_n_frames = 60i64; // every 2 s at 30 fps
                                             // Use a 5-second ring buffer with a generous memory cap (1 GB).
        let buffer_duration = Duration::from_secs(5);
        let mut buffer = ReplayBuffer::with_params(buffer_duration, 1024);

        // Push 10 seconds worth of frames (300 frames total).
        // pts is in QPC units: frame_i * (qpc / fps) ≈ real wall-clock time.
        let step = qpc / fps; // QPC units per frame
        for i in 0i64..300 {
            let pts = i * step;
            let is_kf = i % keyframe_every_n_frames == 0;
            buffer.push(create_test_packet(pts, is_kf, 1024));

            // Once the buffer has been running for more than 5 s (i > 150),
            // eviction should be active and keyframes must remain present.
            if i > 150 {
                let kf_count = buffer.stats().keyframe_count;
                assert!(
                    kf_count > 0,
                    "keyframe_count dropped to 0 at frame {} (duration-based eviction bug)",
                    i
                );
            }
        }

        // Final sanity: snapshot_from should return packets starting at a keyframe.
        let newest_pts = buffer.newest_pts().unwrap();
        let start_pts = newest_pts - qpc * 5; // last 5 s
        let snap = buffer
            .snapshot_from(start_pts)
            .expect("snapshot_from failed after buffer wrap");
        assert!(!snap.is_empty(), "snapshot_from returned empty after wrap");
        assert!(
            snap.iter().any(|p| p.is_keyframe),
            "snapshot after wrap contains no keyframe packets"
        );
    }
}
