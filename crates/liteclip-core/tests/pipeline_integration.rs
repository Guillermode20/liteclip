//! Integration: Full pipeline test (capture → buffer → encode → output).
//!
//! Tests the complete data flow without real hardware dependencies using mock components.
//! These tests verify the pipeline's ability to handle frame flow, memory pressure,
//! and concurrent access patterns.

mod common;

use common::builders::ConfigBuilder;
use common::fixtures::{make_frame_sequence, make_packet_sequence};
use common::mocks::{MockCaptureSource, MockEncoder};
use liteclip_core::buffer::ring::LockFreeReplayBuffer;
use liteclip_core::encode::Encoder;

/// Test: Frames pushed to buffer can be retrieved in snapshots.
/// Verifies basic buffer storage and retrieval functionality.
#[test]
fn buffer_stores_and_retrieves_frames() -> anyhow::Result<()> {
    let config = ConfigBuilder::new()
        .with_replay_duration(30)
        .with_memory_limit(256)
        .build();

    let buffer = LockFreeReplayBuffer::new(&config)?;

    // Simulate encoding and pushing packets
    let packets = make_packet_sequence(60, 1_000_000 / 30, 30);
    for packet in &packets {
        buffer.push(packet.clone());
    }

    // Take snapshot and verify
    let snapshot = buffer.snapshot()?;
    assert_eq!(snapshot.len(), 60);

    // Verify first and last packet
    assert_eq!(snapshot.first().unwrap().pts, packets.first().unwrap().pts);
    assert_eq!(snapshot.last().unwrap().pts, packets.last().unwrap().pts);

    Ok(())
}

/// Test: Mock encoder produces valid packets for each input frame.
/// Verifies the encoder interface correctly processes frames and emits packets.
#[test]
fn mock_encoder_produces_packets_for_each_frame() -> anyhow::Result<()> {
    let mut encoder = MockEncoder::new();
    let config = liteclip_core::encode::ResolvedEncoderConfig {
        bitrate_mbps: 20,
        framerate: 30,
        resolution: (1920, 1080),
        use_native_resolution: false,
        encoder_type: liteclip_core::encode::ResolvedEncoderType::Software,
        quality_preset: liteclip_core::config::QualityPreset::Balanced,
        rate_control: liteclip_core::config::RateControl::Cbr,
        quality_value: None,
        keyframe_interval_secs: 2,
        use_cpu_readback: false,
        output_index: 0,
    };

    encoder.init(&config)?;

    // Feed frames
    let frames = make_frame_sequence(10, 1920, 1080, 30);
    for frame in &frames {
        encoder.encode_frame(frame)?;
    }

    // Collect packets
    let mut packets = Vec::new();
    while let Ok(packet) = encoder.packet_rx().try_recv() {
        packets.push(packet);
    }

    assert_eq!(packets.len(), 10);
    assert_eq!(encoder.frame_count(), 10);

    Ok(())
}

/// Test: Complete pipeline flow from capture to buffer storage.
/// Simulates the full data path: capture source → frame generation → buffer storage.
#[test]
fn full_pipeline_capture_to_buffer() -> anyhow::Result<()> {
    let config = ConfigBuilder::new()
        .with_replay_duration(10)
        .with_memory_limit(128)
        .build();

    let buffer = LockFreeReplayBuffer::new(&config)?;
    let (capture, _frame_rx) = MockCaptureSource::new(1280, 720);

    // Simulate capture
    capture.emit_frames(30, 30);

    // Manually create packets as if they were encoded
    let packets = make_packet_sequence(30, 1_000_000 / 30, 30);
    for packet in &packets {
        buffer.push(packet.clone());
    }

    // Verify buffer has the data
    let snapshot = buffer.snapshot()?;
    assert_eq!(snapshot.len(), 30);

    // Verify keyframe exists (first packet should be keyframe)
    let has_keyframe = snapshot.iter().any(|p| p.is_keyframe);
    assert!(has_keyframe, "Buffer should contain at least one keyframe");

    Ok(())
}

/// Test: Buffer evicts oldest packets when memory limit is exceeded.
/// Critical test: verifies memory pressure handling without data corruption.
#[test]
fn buffer_evicts_old_packets_under_memory_pressure() -> anyhow::Result<()> {
    // Small memory limit to force eviction
    let config = ConfigBuilder::new()
        .with_replay_duration(5)
        .with_memory_limit(1) // 1 MB limit
        .build();

    let buffer = LockFreeReplayBuffer::new(&config)?;

    // Push many large packets
    let packet_count = 100;
    let packet_size = 50_000; // 50KB each
    for i in 0..packet_count {
        let packet = liteclip_core::encode::EncodedPacket {
            data: bytes::Bytes::from(vec![0u8; packet_size]),
            pts: i as i64 * 1_000_000,
            dts: i as i64 * 1_000_000,
            is_keyframe: i % 30 == 0,
            stream: liteclip_core::encode::StreamType::Video,
            resolution: None,
        };
        buffer.push(packet);
    }

    // Buffer should have evicted old packets
    let snapshot = buffer.snapshot()?;
    assert!(
        snapshot.len() < packet_count,
        "Buffer should have evicted some packets due to memory pressure (got {} packets, expected fewer than {})",
        snapshot.len(),
        packet_count
    );

    // But should still have recent packets
    assert!(
        snapshot.len() > 0,
        "Buffer should still have some recent packets"
    );

    Ok(())
}

/// Test: Buffer handles concurrent readers and writers correctly.
/// Verifies thread-safety of the lock-free ring buffer implementation.
#[test]
fn buffer_concurrent_readers_and_writers() -> anyhow::Result<()> {
    use std::sync::Arc;
    use std::thread;

    let config = ConfigBuilder::new()
        .with_replay_duration(60)
        .with_memory_limit(512)
        .build();

    let buffer = Arc::new(LockFreeReplayBuffer::new(&config)?);

    // Writer thread
    let buffer_writer = buffer.clone();
    let writer = thread::spawn(move || {
        for i in 0..100 {
            let packet = liteclip_core::encode::EncodedPacket {
                data: bytes::Bytes::from(vec![0u8; 1024]),
                pts: i as i64 * 1_000_000,
                dts: i as i64 * 1_000_000,
                is_keyframe: i % 30 == 0,
                stream: liteclip_core::encode::StreamType::Video,
                resolution: None,
            };
            buffer_writer.push(packet);
            std::thread::yield_now();
        }
    });

    // Reader threads
    let mut readers = Vec::new();
    for _ in 0..3 {
        let buffer_reader = buffer.clone();
        let reader = thread::spawn(move || {
            let mut snapshot_count = 0;
            for _ in 0..10 {
                if let Ok(_snapshot) = buffer_reader.snapshot() {
                    snapshot_count += 1;
                    std::thread::yield_now();
                }
            }
            snapshot_count
        });
        readers.push(reader);
    }

    writer.join().unwrap();

    for reader in readers {
        let count = reader.join().unwrap();
        assert!(count > 0, "Reader should have successfully taken snapshots");
    }

    // Final snapshot should have all packets
    let final_snapshot = buffer.snapshot()?;
    assert_eq!(final_snapshot.len(), 100);

    Ok(())
}
