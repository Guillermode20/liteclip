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
        assert_eq!(buffer.packets.len(), 10);
        assert_eq!(buffer.keyframe_index.len(), 10);
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
        assert!(buffer.total_bytes <= buffer.max_memory_bytes);
        assert!(buffer.packets.len() < 100);
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
        assert!(buffer.packets.is_empty());
        assert!(buffer.keyframe_index.is_empty());
        assert_eq!(buffer.total_bytes, 0);
        assert_eq!(buffer.base_offset, 0);
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
        assert!(buffer.packets.len() < 200);
        assert!(buffer.base_offset > 0);
        let snap = buffer.snapshot_from(last_keyframe_pts).unwrap();
        assert!(!snap.is_empty());
        assert!(snap[0].pts <= last_keyframe_pts + 10_000_000);
    }
}
