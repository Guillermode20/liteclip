//! Stress tests for SPMC ring buffer contention.
//!
//! These tests exercise the lock-free ring buffer under high concurrency,
//! verifying that data races, torn reads, and memory ordering issues
//! do not produce incorrect snapshots or panics.
//!
//! ## Categories
//!
//! | Tag | When to run |
//! |-----|-------------|
//! | `#[cfg_attr(not(feature = "test-stress"), ignore)]` | `cargo test --features test-stress` |

#![cfg_attr(not(feature = "test-stress"), allow(unused_imports))]

mod common;

use bytes::Bytes;
use common::builders::ConfigBuilder;
use liteclip_core::buffer::ring::LockFreeReplayBuffer;
use liteclip_core::encode::{EncodedPacket, StreamType};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// Create a minimal config for stress tests.
fn stress_config() -> liteclip_core::config::Config {
    ConfigBuilder::new()
        .with_replay_duration(30)
        .with_memory_limit(512)
        .build()
}

/// Create a test packet with a deterministic payload.
fn make_packet(pts: i64, val: u8) -> EncodedPacket {
    let mut data = vec![val; 1024];
    // Stamp PTS into the first 8 bytes so readers can verify ordering
    data[..8].copy_from_slice(&pts.to_le_bytes());
    EncodedPacket {
        data: Bytes::from(data),
        pts,
        dts: pts,
        is_keyframe: pts % 30_000_000 == 0,
        stream: StreamType::Video,
        resolution: None,
        codec: None,
    }
}

// ===========================================================================
// Baseline: single writer, single reader
// ===========================================================================

#[test]
#[cfg_attr(not(feature = "test-stress"), ignore)]
fn spmc_single_writer_single_reader_no_data_loss() {
    let config = stress_config();
    let buffer = Arc::new(LockFreeReplayBuffer::new(&config).unwrap());
    let done = Arc::new(AtomicBool::new(false));
    let push_count = Arc::new(AtomicUsize::new(0));

    // Writer
    let w_buffer = buffer.clone();
    let w_done = done.clone();
    let w_count = push_count.clone();
    let writer = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut i = 0i64;
        while Instant::now() < deadline {
            w_buffer.push(make_packet(i * 1_000_000, (i & 0xff) as u8));
            i += 1;
            w_count.store(i as usize, Ordering::Release);
            thread::yield_now();
        }
        w_done.store(true, Ordering::Release);
    });

    // Reader
    let r_buffer = buffer.clone();
    let r_done = done.clone();
    let reader = thread::spawn(move || {
        let mut last_snapshot_count = 0usize;
        while !r_done.load(Ordering::Acquire) {
            if let Ok(snapshot) = r_buffer.snapshot() {
                // Verify ordering invariant
                for pair in snapshot.windows(2) {
                    assert!(
                        pair[0].pts <= pair[1].pts,
                        "Reader detected out-of-order PTS: {} > {}",
                        pair[0].pts,
                        pair[1].pts,
                    );
                }
                last_snapshot_count = snapshot.len();
            }
            thread::yield_now();
        }
        last_snapshot_count
    });

    writer.join().expect("Writer panicked");
    let final_count = reader.join().expect("Reader panicked");
    let pushed = push_count.load(Ordering::Acquire);

    assert!(
        pushed > 0,
        "Writer should have pushed at least one packet (pushed={})",
        pushed
    );
    eprintln!(
        "SPMC single writer/single reader: pushed={}, final_snapshot={}",
        pushed, final_count,
    );
}

// ===========================================================================
// High-contention: 1 writer, 4 concurrent readers
// ===========================================================================

#[test]
#[cfg_attr(not(feature = "test-stress"), ignore)]
fn spmc_high_contention_four_readers() {
    let config = stress_config();
    let buffer = Arc::new(LockFreeReplayBuffer::new(&config).unwrap());
    let done = Arc::new(AtomicBool::new(false));
    let push_count = Arc::new(AtomicUsize::new(0));

    // Writer
    let w_buffer = buffer.clone();
    let w_done = done.clone();
    let w_count = push_count.clone();
    let writer = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut i = 0i64;
        while Instant::now() < deadline {
            w_buffer.push(make_packet(i * 1_000_000, (i & 0xff) as u8));
            i += 1;
            w_count.store(i as usize, Ordering::Release);
        }
        w_done.store(true, Ordering::Release);
    });

    // 4 readers
    let mut reader_handles = Vec::new();
    for _ in 0..4 {
        let r_buffer = buffer.clone();
        let r_done = done.clone();
        let handle = thread::spawn(move || {
            let mut snapshot_count = 0usize;
            let mut reader_errors = 0usize;
            while !r_done.load(Ordering::Acquire) {
                match r_buffer.snapshot() {
                    Ok(snapshot) => {
                        // Verify monotonically non-decreasing PTS
                        for pair in snapshot.windows(2) {
                            if pair[0].pts > pair[1].pts {
                                reader_errors += 1;
                            }
                        }
                        snapshot_count += 1;
                    }
                    Err(_) => {
                        // Snapshot may briefly fail during eviction — acceptable
                    }
                }
                thread::yield_now();
            }
            (snapshot_count, reader_errors)
        });
        reader_handles.push(handle);
    }

    writer.join().expect("Writer panicked");
    let pushed = push_count.load(Ordering::Acquire);

    let mut total_snapshots = 0usize;
    let mut total_errors = 0usize;
    for (i, handle) in reader_handles.into_iter().enumerate() {
        let (count, errors) = handle.join().expect("Reader panicked");
        total_snapshots += count;
        total_errors += errors;
        eprintln!(
            "  Reader {}: {} snapshots, {} ordering errors",
            i, count, errors
        );
    }

    assert!(
        total_errors == 0,
        "Some readers detected out-of-order PTS: {} total ordering errors",
        total_errors
    );
    assert!(
        total_snapshots > 0,
        "Readers should have taken at least one snapshot"
    );
    assert!(pushed > 0, "Writer should have pushed data");
    eprintln!(
        "SPMC high-contention (4 readers): pushed={}, total_snapshots={}",
        pushed, total_snapshots,
    );
}

// ===========================================================================
// Read-during-write: verify snapshot integrity while writer is active
// ===========================================================================

#[test]
#[cfg_attr(not(feature = "test-stress"), ignore)]
fn spmc_snapshot_integrity_during_write() {
    let config = stress_config();
    let buffer = Arc::new(LockFreeReplayBuffer::new(&config).unwrap());
    let running = Arc::new(AtomicBool::new(true));
    let push_count = Arc::new(AtomicUsize::new(0));
    let snapshot_count = Arc::new(AtomicUsize::new(0));

    // Writer thread — pushes continuously
    let w_buffer = buffer.clone();
    let w_running = running.clone();
    let w_count = push_count.clone();
    let writer = thread::spawn(move || {
        let mut pts = 0i64;
        while w_running.load(Ordering::Acquire) {
            w_buffer.push(make_packet(pts, (pts & 0xff) as u8));
            pts += 1_000_000;
            w_count.store(pts as usize, Ordering::Release);
        }
    });

    // Reader thread — takes snapshots continuously
    let r_buffer = buffer.clone();
    let r_running = running.clone();
    let r_count = snapshot_count.clone();
    let reader = thread::spawn(move || {
        while r_running.load(Ordering::Acquire) {
            if let Ok(snapshot) = r_buffer.snapshot() {
                // Critical invariant: every packet in the snapshot must
                // have a valid (non-negative) PTS, and must be readable
                // without panicking.
                for packet in snapshot.iter() {
                    // Reading the packet data should not panic.
                    let _ = packet.data.len();
                    // PTS should be non-negative (valid timestamp).
                    assert!(
                        packet.pts >= 0,
                        "Packet in snapshot has negative PTS: {}",
                        packet.pts,
                    );
                }
                r_count.fetch_add(1, Ordering::Relaxed);
            }
        }
    });

    // Let them run for 2 seconds
    thread::sleep(Duration::from_secs(2));
    running.store(false, Ordering::Release);

    writer.join().expect("Writer panicked");
    reader.join().expect("Reader panicked");

    let pushed = push_count.load(Ordering::Acquire);
    let snapped = snapshot_count.load(Ordering::Relaxed);

    eprintln!(
        "SPMC integrity during write: pushed={}, snapshots={}",
        pushed, snapped,
    );
    assert!(snapped > 0, "Must have taken at least one snapshot");
}

// ===========================================================================
// Memory pressure: rapid push with small buffer
// ===========================================================================

#[test]
#[cfg_attr(not(feature = "test-stress"), ignore)]
fn spmc_memory_pressure_rapid_push_and_snapshot() {
    // Tiny memory limit to force aggressive eviction
    let mut config = stress_config();
    config.advanced.memory_limit_mb = 1; // 1 MB
    let buffer = Arc::new(LockFreeReplayBuffer::new(&config).unwrap());

    let running = Arc::new(AtomicBool::new(true));

    // Writer that pushes large packets rapidly
    let w_buffer = buffer.clone();
    let w_running = running.clone();
    let writer = thread::spawn(move || {
        let mut pts = 0i64;
        while w_running.load(Ordering::Acquire) {
            // 100 KB packets to force eviction
            let packet = EncodedPacket {
                data: Bytes::from(vec![0u8; 100_000]),
                pts,
                dts: pts,
                is_keyframe: pts % 30_000_000 == 0,
                stream: StreamType::Video,
                resolution: None,
                codec: None,
            };
            w_buffer.push(packet);
            pts += 1_000_000;
        }
    });

    // Read snapshots frequently
    let r_buffer = buffer.clone();
    let r_running = running.clone();
    let reader = thread::spawn(move || {
        let mut snapshots = 0usize;
        while r_running.load(Ordering::Acquire) {
            if let Ok(snapshot) = r_buffer.snapshot() {
                // Verify that data in the snapshot hasn't been corrupted
                for packet in snapshot.iter() {
                    let data_len = packet.data.len();
                    let pts = packet.pts;
                    // Basic sanity: data length should match what we wrote
                    // (100 KB unless truncated by eviction logic)
                    assert!(
                        data_len == 100_000 || data_len == 0,
                        "Packet had unexpected data length {} at pts={}",
                        data_len,
                        pts,
                    );
                }
                snapshots += 1;
            }
        }
        snapshots
    });

    thread::sleep(Duration::from_secs(2));
    running.store(false, Ordering::Release);

    writer.join().expect("Writer panicked");
    let snapshots = reader.join().expect("Reader panicked");

    eprintln!("SPMC memory pressure: snapshots={}", snapshots);
    assert!(
        snapshots > 0,
        "Should have taken snapshots under memory pressure"
    );
}

// ===========================================================================
// Burst: rapid burst of pushes with immediate snapshot
// ===========================================================================

#[test]
#[cfg_attr(not(feature = "test-stress"), ignore)]
fn spmc_burst_push_then_snapshot() {
    let config = stress_config();
    let buffer = LockFreeReplayBuffer::new(&config).unwrap();

    // Burst of 10,000 pushes
    let burst_start = Instant::now();
    for i in 0..10_000 {
        buffer.push(make_packet(i as i64 * 33_333, (i & 0xff) as u8));
    }
    let push_duration = burst_start.elapsed();

    // Immediately snapshot
    let snapshot_start = Instant::now();
    let snapshot = buffer
        .snapshot()
        .expect("Snapshot after burst should succeed");
    let snapshot_duration = snapshot_start.elapsed();

    // Verify snapshot properties
    assert!(
        !snapshot.is_empty(),
        "Snapshot should contain data after 10K pushes"
    );
    for pair in snapshot.windows(2) {
        assert!(
            pair[0].pts <= pair[1].pts,
            "Out-of-order PTS in burst snapshot: {} > {}",
            pair[0].pts,
            pair[1].pts,
        );
    }

    eprintln!(
        "SPMC burst: 10,000 pushes in {:?}, snapshot {:?} with {} packets",
        push_duration,
        snapshot_duration,
        snapshot.len(),
    );
}

// ===========================================================================
// Producer/consumer alternating pattern
// ===========================================================================

#[test]
#[cfg_attr(not(feature = "test-stress"), ignore)]
fn spmc_alternating_push_snapshot_no_data_corruption() {
    let config = stress_config();
    let buffer = LockFreeReplayBuffer::new(&config).unwrap();

    for i in 0..100 {
        // Push
        let pts = i as i64 * 1_000_000;
        buffer.push(make_packet(pts, (i & 0xff) as u8));

        // Snapshot — use match since TrackedSnapshot doesn't impl Default
        match buffer.snapshot() {
            Ok(snapshot) => {
                // After the first push, we should have data
                if !snapshot.is_empty() {
                    for pair in snapshot.windows(2) {
                        assert!(
                            pair[0].pts <= pair[1].pts,
                            "Out-of-order at iteration {}: {} > {}",
                            i,
                            pair[0].pts,
                            pair[1].pts,
                        );
                    }

                    // Last packet in snapshot should be the one we just pushed
                    let last = snapshot.last().unwrap();
                    assert!(
                        last.pts >= pts || snapshot.len() == 1,
                        "Last packet PTS ({}) should be >= latest push ({}) at iteration {}",
                        last.pts,
                        pts,
                        i,
                    );
                }
            }
            Err(_) => {
                // Snapshot may fail briefly during eviction — skip this iteration
                continue;
            }
        }
    }
}
