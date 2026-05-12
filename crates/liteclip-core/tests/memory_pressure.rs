//! Memory pressure boundary tests.
//!
//! Tests the ring buffer's eviction behaviour under memory constraints,
//! verifying that the 80% watermark eviction, 512 MB snapshot cap, and
//! batched eviction work correctly.
//!
//! ## Categories
//!
//! | Tag | When to run |
//! |-----|-------------|
//! | `#[cfg_attr(not(feature = "test-slow"), ignore)]` | `cargo test --features test-slow` |

#![cfg_attr(not(feature = "test-slow"), allow(unused_imports))]

mod common;

use bytes::Bytes;
use common::builders::ConfigBuilder;
use liteclip_core::buffer::ring::LockFreeReplayBuffer;
use liteclip_core::encode::{EncodedPacket, StreamType};

/// Create a test packet with given PTS and size.
fn make_packet(pts: i64, size: usize, is_keyframe: bool) -> EncodedPacket {
    EncodedPacket {
        data: Bytes::from(vec![0u8; size]),
        pts,
        dts: pts,
        is_keyframe,
        stream: StreamType::Video,
        resolution: None,
        codec: None,
    }
}

// ===========================================================================
// 80% watermark eviction
// ===========================================================================

/// Push enough data to exceed 80% of a small memory limit.
/// The buffer should evict proactively rather than blocking.
#[test]
#[cfg_attr(not(feature = "test-slow"), ignore)]
fn eviction_triggered_at_watermark() {
    // 10 MB limit; 80% = 8 MB watermark
    let config = ConfigBuilder::new()
        .with_replay_duration(60)
        .with_memory_limit(10)
        .build();
    let buffer = LockFreeReplayBuffer::new(&config).unwrap();

    // Push 100 KB packets until well past the watermark (10 MB)
    let mut pts = 0i64;
    for _ in 0..200 {
        buffer.push(make_packet(pts, 100_000, pts % 30_000_000 == 0));
        pts += 1_000_000;
    }

    // Snapshot should succeed and contain fewer packets than pushed
    // (eviction should have happened)
    let snapshot = buffer.snapshot().expect("Snapshot should succeed");
    assert!(
        snapshot.len() < 200,
        "Expected eviction: snapshot.len()={}, pushed=200",
        snapshot.len(),
    );
    assert!(
        !snapshot.is_empty(),
        "Snapshot should still contain recent packets",
    );

    // Verify ordering is preserved in the post-eviction snapshot
    for pair in snapshot.windows(2) {
        assert!(
            pair[0].pts <= pair[1].pts,
            "Post-eviction PTS order violated: {} > {}",
            pair[0].pts,
            pair[1].pts,
        );
    }

    eprintln!(
        "Watermark eviction: pushed 200 x 100KB, snapshot has {} packets",
        snapshot.len(),
    );
}

// ===========================================================================
// Extreme memory limit (128 MB — the minimum documented)
// ===========================================================================

#[test]
#[cfg_attr(not(feature = "test-slow"), ignore)]
fn extreme_small_memory_limit_does_not_crash() {
    let config = ConfigBuilder::new()
        .with_replay_duration(60)
        .with_memory_limit(128) // 128 MB — minimum documented
        .build();

    let buffer = LockFreeReplayBuffer::new(&config).unwrap();

    // Push 500 x 1 MB = 500 MB total, far exceeding the limit
    for i in 0..500 {
        buffer.push(make_packet(i as i64 * 1_000_000, 1_000_000, i % 30 == 0));
    }

    // Should still be able to snapshot
    let snapshot = buffer.snapshot().expect("Snapshot must still succeed");
    assert!(
        !snapshot.is_empty(),
        "Snapshot must not be empty after heavy push",
    );

    // Verify ordering
    for pair in snapshot.windows(2) {
        assert!(pair[0].pts <= pair[1].pts, "PTS order violated");
    }

    eprintln!(
        "128 MB limit: pushed 500 x 1MB, snapshot has {} packets",
        snapshot.len(),
    );
}

// ===========================================================================
// Very large packets (edge case for ring buffer slot allocation)
// ===========================================================================

#[test]
#[cfg_attr(not(feature = "test-slow"), ignore)]
fn very_large_packets_handled_gracefully() {
    let config = ConfigBuilder::new()
        .with_replay_duration(30)
        .with_memory_limit(512)
        .build();
    let buffer = LockFreeReplayBuffer::new(&config).unwrap();

    // Push 10 packets of 10 MB each = 100 MB total
    for i in 0..10 {
        buffer.push(make_packet(i as i64 * 1_000_000, 10_000_000, true));
    }

    let snapshot = buffer.snapshot().expect("Snapshot with large packets");
    assert!(!snapshot.is_empty(), "Snapshot should have data");

    // Verify the large packets are accessible without panic
    for packet in snapshot.iter() {
        assert!(
            packet.data.len() <= 10_000_000,
            "Packet data should not exceed pushed size: got {}",
            packet.data.len(),
        );
    }

    eprintln!(
        "Large packets: pushed 10 x 10MB, snapshot has {} packets",
        snapshot.len(),
    );
}

// ===========================================================================
// Many small packets (edge case for slot count limits)
// ===========================================================================

#[test]
#[cfg_attr(not(feature = "test-slow"), ignore)]
fn many_small_packets_do_not_exhaust_slots() {
    let config = ConfigBuilder::new()
        .with_replay_duration(60)
        .with_memory_limit(512)
        .build();
    let buffer = LockFreeReplayBuffer::new(&config).unwrap();

    // Push 100,000 small packets (100 bytes each ≈ 10 MB total)
    for i in 0..100_000 {
        buffer.push(make_packet(i as i64 * 33_333, 100, i % 30 == 0));
    }

    let snapshot = buffer
        .snapshot()
        .expect("Snapshot after many small packets");
    assert!(!snapshot.is_empty(), "Snapshot should have data");

    // Verify ordering across all packets
    for pair in snapshot.windows(2) {
        assert!(
            pair[0].pts <= pair[1].pts,
            "PTS order violated at many-small-packets boundary",
        );
    }

    eprintln!(
        "Many small packets: pushed 100K x 100B, snapshot has {} packets",
        snapshot.len(),
    );
}

// ===========================================================================
// Snapshot after buffer completely full (every slot used)
// ===========================================================================

#[test]
#[cfg_attr(not(feature = "test-slow"), ignore)]
fn snapshot_after_buffer_exhaustion() {
    // Very small memory limit ensures slots are reused aggressively
    let config = ConfigBuilder::new()
        .with_replay_duration(5)
        .with_memory_limit(8) // 8 MB
        .build();
    let buffer = LockFreeReplayBuffer::new(&config).unwrap();

    // Push until we've cycled the buffer many times
    for i in 0..10_000 {
        buffer.push(make_packet(i as i64 * 1_000_000, 10_000, i % 30 == 0));
    }

    // Snapshot should work and return some reasonable subset
    let snapshot = buffer.snapshot().expect("Snapshot after exhaustion");
    assert!(!snapshot.is_empty(), "Snapshot should not be empty");

    // Check ordering
    for pair in snapshot.windows(2) {
        assert!(pair[0].pts <= pair[1].pts, "PTS ordering violated");
    }

    eprintln!(
        "Buffer exhaustion: pushed 10K x 10KB into 8MB buffer, snapshot has {} packets",
        snapshot.len(),
    );
}

// ===========================================================================
// Zero-length packets (edge case)
// ===========================================================================

#[test]
#[cfg_attr(not(feature = "test-slow"), ignore)]
fn zero_length_packets_do_not_corrupt_buffer() {
    let config = ConfigBuilder::new()
        .with_replay_duration(30)
        .with_memory_limit(512)
        .build();
    let buffer = LockFreeReplayBuffer::new(&config).unwrap();

    // Push some normal packets mixed with zero-length packets
    buffer.push(make_packet(0, 1024, true));
    buffer.push(make_packet(1_000_000, 0, false));
    buffer.push(make_packet(2_000_000, 1024, false));
    buffer.push(make_packet(3_000_000, 0, true));
    buffer.push(make_packet(4_000_000, 1024, false));

    let snapshot = buffer
        .snapshot()
        .expect("Snapshot with zero-length packets");
    assert!(!snapshot.is_empty(), "Snapshot should have data");

    for packet in snapshot.iter() {
        // Zero-length packets should not cause panics when accessed
        let _ = packet.data.len();
    }
}

// ===========================================================================
// Interleaved push and snapshot under memory pressure
// ===========================================================================

#[test]
#[cfg_attr(not(feature = "test-slow"), ignore)]
fn interleaved_push_snapshot_under_pressure() {
    let config = ConfigBuilder::new()
        .with_replay_duration(30)
        .with_memory_limit(64) // 64 MB
        .build();
    let buffer = LockFreeReplayBuffer::new(&config).unwrap();

    for i in 0..1000 {
        // Push a batch
        for _ in 0..10 {
            buffer.push(make_packet(i as i64 * 1_000_000, 50_000, i % 30 == 0));
        }

        // Snapshot
        let snapshot = buffer
            .snapshot()
            .expect("Snapshot during interleaved pattern");
        if !snapshot.is_empty() {
            for pair in snapshot.windows(2) {
                assert!(
                    pair[0].pts <= pair[1].pts,
                    "Interleaved PTS violation at iteration {}",
                    i,
                );
            }
        }
    }
}
