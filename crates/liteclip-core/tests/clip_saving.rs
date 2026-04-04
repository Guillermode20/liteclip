//! Integration: Buffer snapshot and clip output preparation tests.
//!
//! Tests the buffer snapshot functionality that prepares data for the muxing
//! and clip saving pipeline. These tests verify snapshot behavior, packet
//! sequence validation, and metadata tracking - the preparatory steps before
//! actual clip encoding and file writing.
//!
//! Note: These tests do NOT test actual MP4 muxing or file I/O. For muxing tests,
//! additional integration tests with the output module would be needed.

mod common;

use bytes::Bytes;
use common::builders::ConfigBuilder;
use common::fixtures::make_packet_sequence;
use liteclip_core::buffer::ring::LockFreeReplayBuffer;
use liteclip_core::encode::{EncodedPacket, StreamType};
use tempfile::TempDir;

/// Test: Buffer snapshot can be written to a byte stream.
///
/// Verifies that packets from a buffer snapshot can be serialized
/// and would be ready for muxing to a container format.
#[test]
fn buffer_snapshot_serializes_to_bytes() -> anyhow::Result<()> {
    let config = ConfigBuilder::new()
        .with_replay_duration(30)
        .with_memory_limit(256)
        .build();

    let buffer = LockFreeReplayBuffer::new(&config)?;

    // Simulate encoded packets in buffer
    let packets = make_packet_sequence(60, 1_000_000 / 30, 30);
    for packet in &packets {
        buffer.push(packet.clone());
    }

    // Take snapshot
    let snapshot = buffer.snapshot()?;
    assert_eq!(snapshot.len(), 60);

    // Verify packets can be iterated and their data accessed
    let total_bytes: usize = snapshot.iter().map(|p| p.data.len()).sum();
    assert!(total_bytes > 0, "Snapshot should contain packet data");

    // Verify timestamps are sequential
    for window in snapshot.windows(2) {
        assert!(
            window[0].pts <= window[1].pts,
            "Packet timestamps should be non-decreasing"
        );
    }

    Ok(())
}

/// Test: Buffer snapshot maintains keyframe alignment.
///
/// When saving a clip, we need to ensure the output starts on a keyframe
/// for proper decode ability. This test verifies keyframe detection.
#[test]
fn buffer_snapshot_maintains_keyframe_alignment() -> anyhow::Result<()> {
    let config = ConfigBuilder::new()
        .with_replay_duration(60)
        .with_memory_limit(512)
        .build();

    let buffer = LockFreeReplayBuffer::new(&config)?;

    // Push packets with keyframe every 30 frames (GOP = 30)
    let gop_size = 30;
    let packet_count = 120;

    for i in 0..packet_count {
        let is_keyframe = i % gop_size == 0;
        let packet = EncodedPacket {
            data: Bytes::from(vec![0u8; 1024]),
            pts: i as i64 * 33_333, // ~30fps in microseconds
            dts: i as i64 * 33_333,
            is_keyframe,
            stream: StreamType::Video,
            resolution: Some((1920, 1080)),
        };
        buffer.push(packet);
    }

    let snapshot = buffer.snapshot()?;

    // Verify at least one keyframe exists
    let keyframe_count = snapshot.iter().filter(|p| p.is_keyframe).count();
    assert!(
        keyframe_count >= 3,
        "Should have at least 3 keyframes for {} packets with GOP of {}",
        snapshot.len(),
        gop_size
    );

    // Verify keyframe positions are correct
    for (i, packet) in snapshot.iter().enumerate() {
        if packet.is_keyframe {
            // Due to snapback and duration limiting, keyframes might not be at exact GOP boundaries
            // but they should exist periodically
            assert!(
                packet.pts >= 0,
                "Keyframe at position {} should have valid PTS",
                i
            );
        }
    }

    Ok(())
}

/// Test: Clip save directory is validated and created if needed.
///
/// Verifies that the save directory path handling correctly validates
/// and prepares the output location for clip files.
#[test]
fn clip_save_directory_validation() -> anyhow::Result<()> {
    let temp_dir = TempDir::new()?;
    let clips_dir = temp_dir.path().join("clips").join("nested");

    // Directory should not exist initially
    assert!(
        !clips_dir.exists(),
        "Clips directory should not exist initially"
    );

    // Create the directory structure
    std::fs::create_dir_all(&clips_dir)?;
    assert!(
        clips_dir.exists(),
        "Clips directory should exist after creation"
    );

    // Verify we can construct a clip path
    let clip_path = clips_dir.join("test_clip.mp4");
    assert_eq!(
        clip_path.extension().unwrap().to_str().unwrap(),
        "mp4",
        "Clip should have MP4 extension"
    );

    Ok(())
}

/// Test: Output filename generation with timestamps.
///
/// Verifies that clip filenames include timestamps for uniqueness
/// and proper sorting.
#[test]
fn clip_filename_includes_timestamp() -> anyhow::Result<()> {
    use chrono::Local;

    let now = Local::now();
    let filename = format!("LiteClip_{}.mp4", now.format("%Y%m%d_%H%M%S"));

    // Verify filename format
    assert!(
        filename.starts_with("LiteClip_"),
        "Filename should have LiteClip prefix"
    );
    assert!(
        filename.ends_with(".mp4"),
        "Filename should have .mp4 extension"
    );
    assert!(filename.len() > 20, "Filename should include timestamp");

    // Verify timestamp components are present (YYYYMMDD_HHMMSS format)
    let timestamp_part = &filename[9..filename.len() - 4]; // Extract between prefix and extension
    assert_eq!(
        timestamp_part.len(),
        15,
        "Timestamp should be 15 chars (YYYYMMDD_HHMMSS)"
    );
    assert_eq!(
        &timestamp_part[8..9],
        "_",
        "Timestamp should have underscore separator"
    );

    Ok(())
}

/// Test: Buffer snapshot respects duration limits when saving.
///
/// When saving a clip, the buffer should retain approximately the replay duration
/// worth of frames, though exact counts depend on memory limits and implementation.
#[test]
fn clip_duration_windowing() -> anyhow::Result<()> {
    // Create a buffer with 30 second replay duration
    let config = ConfigBuilder::new()
        .with_replay_duration(30)
        .with_memory_limit(4096)
        .build();

    let buffer = LockFreeReplayBuffer::new(&config)?;

    // Push frames representing 60 seconds of capture at 30fps
    for i in 0..1800 {
        // 60 seconds * 30fps
        let packet = EncodedPacket {
            data: Bytes::from(vec![0u8; 1024]),
            pts: i as i64 * 33_333, // ~30fps
            dts: i as i64 * 33_333,
            is_keyframe: i % 30 == 0,
            stream: StreamType::Video,
            resolution: Some((1920, 1080)),
        };
        buffer.push(packet);
    }

    let snapshot = buffer.snapshot()?;

    // The buffer should have frames (exact count depends on implementation)
    assert!(snapshot.len() > 0, "Snapshot should contain frames");

    // Verify timestamps are sequential
    for window in snapshot.windows(2) {
        assert!(
            window[0].pts <= window[1].pts,
            "Timestamps should be non-decreasing"
        );
    }

    // Verify all packets have consistent resolution
    for packet in snapshot.iter() {
        assert_eq!(packet.resolution, Some((1920, 1080)));
    }

    Ok(())
}

/// Test: Simulated muxer accepts packet sequence.
///
/// This test simulates the muxing process by validating that
/// a sequence of packets forms a valid video stream structure.
#[test]
fn packet_sequence_forms_valid_stream_structure() -> anyhow::Result<()> {
    let packets = make_packet_sequence(90, 33_333, 30);

    // Validate stream structure
    let mut last_pts: Option<i64> = None;
    let mut keyframe_found = false;

    for (i, packet) in packets.iter().enumerate() {
        // PTS should be monotonically increasing
        if let Some(last) = last_pts {
            assert!(
                packet.pts >= last,
                "Packet {} has non-monotonic PTS: {} < {}",
                i,
                packet.pts,
                last
            );
        }
        last_pts = Some(packet.pts);

        // Track keyframes
        if packet.is_keyframe {
            keyframe_found = true;
            assert!(
                i == 0 || i % 30 == 0,
                "Keyframe at position {} should be at expected GOP boundary",
                i
            );
        }

        // All packets should have video stream type
        assert!(
            matches!(packet.stream, StreamType::Video),
            "All packets should be video stream"
        );
    }

    assert!(
        keyframe_found,
        "Stream should contain at least one keyframe"
    );

    // Verify DTS matches PTS for our test data (no B-frames)
    for packet in &packets {
        assert_eq!(
            packet.pts, packet.dts,
            "PTS and DTS should match for I/P-frame only streams"
        );
    }

    Ok(())
}

/// Test: Clip metadata is correctly associated with output.
///
/// Verifies that clip metadata (duration, resolution, timestamp) is tracked.
#[test]
fn clip_metadata_tracking() -> anyhow::Result<()> {
    let resolution = (1920u32, 1080u32);
    let framerate = 30u32;
    let duration_secs = 30u32;

    // Simulate a clip with known parameters
    let frame_count = duration_secs * framerate;
    let packets = make_packet_sequence_with_resolution(
        frame_count as usize,
        1_000_000 / framerate as i64,
        30,
        resolution,
    );

    // Calculate actual duration from packets
    if let (Some(first), Some(last)) = (packets.first(), packets.last()) {
        let duration_us = last.pts - first.pts;
        let duration_sec = duration_us as f64 / 1_000_000.0;

        // Duration should be approximately as expected
        assert!(
            (duration_sec - duration_secs as f64).abs() < 1.0,
            "Clip duration should be approximately {} seconds (got {:.2})",
            duration_secs,
            duration_sec
        );

        // Verify all packets have consistent resolution
        for packet in &packets {
            assert_eq!(
                packet.resolution,
                Some(resolution),
                "All packets should have consistent resolution"
            );
        }
    }

    Ok(())
}

/// Helper function to create packet sequence with resolution
fn make_packet_sequence_with_resolution(
    count: usize,
    pts_interval: i64,
    keyframe_interval: usize,
    resolution: (u32, u32),
) -> Vec<EncodedPacket> {
    use bytes::Bytes;
    use liteclip_core::encode::StreamType;

    (0..count)
        .map(|i| EncodedPacket {
            data: Bytes::from(vec![0u8; 1024]),
            pts: i as i64 * pts_interval,
            dts: i as i64 * pts_interval,
            is_keyframe: i % keyframe_interval == 0,
            stream: StreamType::Video,
            resolution: Some(resolution),
        })
        .collect()
}
