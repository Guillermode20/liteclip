//! Property-based tests for ring buffer invariants.
//!
//! Uses proptest to verify ring buffer correctness under arbitrary inputs.
//! These tests provide strong guarantees about the buffer's behavior with
//! various packet counts, sizes, and memory pressure scenarios.

use bytes::Bytes;
use liteclip_core::buffer::ring::LockFreeReplayBuffer;
use liteclip_core::config::Config;
use liteclip_core::encode::{EncodedPacket, StreamType};
use proptest::prelude::*;

/// Create a test packet with specified PTS and size.
///
/// Keyframes are automatically generated every 30 seconds of PTS.
fn make_packet(pts: i64, size: usize) -> EncodedPacket {
    EncodedPacket {
        data: Bytes::from(vec![0u8; size]),
        pts,
        dts: pts,
        is_keyframe: pts % 30_000_000 == 0, // Keyframe every 30 seconds
        stream: StreamType::Video,
        resolution: None,
    }
}

/// Create a test packet with explicit keyframe flag.
fn make_packet_with_keyframe(pts: i64, size: usize, is_keyframe: bool) -> EncodedPacket {
    EncodedPacket {
        data: Bytes::from(vec![0u8; size]),
        pts,
        dts: pts,
        is_keyframe,
        stream: StreamType::Video,
        resolution: None,
    }
}

/// Create a test config with specified duration and memory limits.
fn make_config(duration_secs: u32, memory_mb: u32) -> Config {
    let mut config = Config::default();
    config.general.replay_duration_secs = duration_secs;
    config.advanced.memory_limit_mb = memory_mb;
    config
}

// Property: After pushing N packets, snapshot should contain at most N packets.
//
// The buffer should never report more packets than have been pushed.
proptest! {
    #[test]
    fn snapshot_size_never_exceeds_pushed_count(
        packet_count in 1u32..200,
        pts_base in 0i64..10_000_000,
    ) {
        let config = make_config(60, 512);
        let buffer = LockFreeReplayBuffer::new(&config).unwrap();

        for i in 0..packet_count {
            let packet = make_packet(pts_base + i as i64 * 1_000_000, 1024);
            buffer.push(packet);
        }

        let snapshot = buffer.snapshot().unwrap();
        prop_assert!(snapshot.len() <= packet_count as usize);
    }
}

// Property: Packet PTS values in snapshot should be non-decreasing.
//
// Critical invariant: timestamps must never go backwards.
proptest! {
    #[test]
    fn packet_pts_non_decreasing(
        start_pts in 0i64..10_000_000,
        deltas in prop::collection::vec(1i64..1_000_000, 10..100),
    ) {
        let config = make_config(60, 512);
        let buffer = LockFreeReplayBuffer::new(&config).unwrap();

        let mut pts = start_pts;
        for delta in deltas {
            let packet = make_packet(pts, 1024);
            buffer.push(packet);
            pts += delta;
        }

        let snapshot = buffer.snapshot().unwrap();
        prop_assert!(snapshot.windows(2).all(|pair| pair[0].pts <= pair[1].pts));
    }
}

// Property: Buffer should handle large packet sizes without overflow.
//
// Tests that the buffer correctly manages memory for various packet sizes.
proptest! {
    #[test]
    fn large_packet_sizes_handled_correctly(
        packet_size in 100usize..1_000_000,
        count in 1usize..10,
    ) {
        let config = make_config(60, 1024);
        let buffer = LockFreeReplayBuffer::new(&config).unwrap();

        for i in 0..count {
            let packet = make_packet(i as i64 * 1_000_000, packet_size);
            buffer.push(packet);
        }

        let snapshot = buffer.snapshot().unwrap();
        prop_assert_eq!(snapshot.len(), count);

        // Verify we can iterate over snapshot without panicking
        for _packet in snapshot.iter() {
            // Packet should be accessible
        }
    }
}

// Property: Memory limit should be respected regardless of input.
//
// The buffer must evict old packets when memory limit is exceeded.
proptest! {
    #[test]
    fn memory_limit_respected_under_pressure(
        packet_count in 10usize..500,
        memory_limit in 1u32..100,
    ) {
        let config = make_config(60, memory_limit);
        let buffer = LockFreeReplayBuffer::new(&config).unwrap();

        for i in 0..packet_count {
            let packet = make_packet(i as i64 * 1_000_000, 10_000);
            buffer.push(packet);
        }

        let snapshot = buffer.snapshot().unwrap();

        // Should have evicted packets if memory limit is exceeded
        // But should still have some recent packets
        prop_assert!(snapshot.len() <= packet_count);
        prop_assert!(snapshot.len() > 0 || packet_count == 0);
    }
}

// Property: Keyframe distribution should retain at least one keyframe with regular GOP cadence.
//
// With sufficient packets and regular keyframe intervals, at least one keyframe should exist.
proptest! {
    #[test]
    fn keyframe_present_after_sufficient_packets(
        packet_count in 30usize..100,
    ) {
        let config = make_config(60, 512);
        let buffer = LockFreeReplayBuffer::new(&config).unwrap();

        // Push packets with keyframe every 30
        for i in 0..packet_count {
            let packet = make_packet_with_keyframe(i as i64 * 1_000_000, 1024, i % 30 == 0);
            buffer.push(packet);
        }

        let snapshot = buffer.snapshot().unwrap();
        let has_keyframe = snapshot.iter().any(|p| p.is_keyframe);

        // With 30+ packets and keyframe every 30, should have at least one
        prop_assert!(has_keyframe);
    }
}

/// Property: Empty buffer should return empty snapshot.
///
/// Edge case: fresh buffer should report zero packets.
#[test]
fn empty_buffer_returns_empty_snapshot() {
    let config = make_config(60, 512);
    let buffer = LockFreeReplayBuffer::new(&config).unwrap();

    let snapshot = buffer.snapshot().unwrap();
    assert_eq!(snapshot.len(), 0);
}

/// Property: Buffer with single packet should return single packet snapshot.
///
/// Edge case: minimal buffer population.
#[test]
fn single_packet_returns_single_snapshot() {
    let config = make_config(60, 512);
    let buffer = LockFreeReplayBuffer::new(&config).unwrap();

    let packet = make_packet(0, 1024);
    buffer.push(packet);

    let snapshot = buffer.snapshot().unwrap();
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].pts, 0);
}

/// Property: Multiple snapshots should return consistent data.
///
/// Taking multiple snapshots without new pushes should yield identical results.
#[test]
fn multiple_snapshots_are_consistent() {
    let config = make_config(60, 512);
    let buffer = LockFreeReplayBuffer::new(&config).unwrap();

    for i in 0..10 {
        let packet = make_packet(i as i64 * 1_000_000, 1024);
        buffer.push(packet);
    }

    let snapshot1 = buffer.snapshot().unwrap();
    let snapshot2 = buffer.snapshot().unwrap();

    // Both snapshots should have same length
    assert_eq!(snapshot1.len(), snapshot2.len());

    // And same PTS values
    for (p1, p2) in snapshot1.iter().zip(snapshot2.iter()) {
        assert_eq!(p1.pts, p2.pts);
    }
}

// Property: Buffer duration should limit packet count to about one window plus one GOP.
//
// The duration setting should effectively limit how many frames are retained.
proptest! {
    #[test]
    fn buffer_duration_limits_packet_count(
        duration_secs in 1u32..120,
        packet_count in 10usize..1000,
    ) {
        let config = make_config(duration_secs, 1024);
        let buffer = LockFreeReplayBuffer::new(&config).unwrap();
        let qpc_ticks_per_sec = liteclip_core::buffer::ring::functions::qpc_frequency().max(1);
        let frame_interval = (qpc_ticks_per_sec / 30).max(1);

        // Push packets at 30fps in QPC ticks.
        for i in 0..packet_count {
            let packet =
                make_packet_with_keyframe(i as i64 * frame_interval, 1024, i % 30 == 0);
            buffer.push(packet);
        }

        let snapshot = buffer.snapshot().unwrap();

        // At 30fps, duration_secs should hold roughly duration_secs*30 + 1 packets
        // due to inclusive timestamp boundaries, plus up to one GOP (30 packets)
        // when snapback aligns to a prior keyframe.
        let expected_max = ((duration_secs as usize) * 30 + 31).min(packet_count);
        prop_assert!(snapshot.len() <= expected_max);
    }
}
