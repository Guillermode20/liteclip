//! Fuzz target for ring buffer invariants.
//!
//! Pushes arbitrary byte slices (interpreted as encoded packets) into the
//! replay buffer and takes snapshots. The fuzzer explores edge cases around
//! packet sizes, PTS values, and keyframe markers.
//!
//! # Running
//!
//! ```bash
//! cargo fuzz run ring_buffer -- -max_len=65536 -timeout=5
//! ```
//!
//! # Corpus
//!
//! Place seed inputs in `fuzz/corpus/ring_buffer/` to guide the fuzzer
//! toward interesting states (e.g., boundary PTS values, empty packets).

#![no_main]

use libfuzzer_sys::fuzz_target;
use liteclip_core::buffer::ring::LockFreeReplayBuffer;
use liteclip_core::config::Config;
use bytes::Bytes;
use liteclip_core::encode::{EncodedPacket, StreamType};

// Create a fixed config for fuzzing — 60s replay, 256 MB limit
fn fuzz_config() -> Config {
    let mut c = Config::default();
    c.general.replay_duration_secs = 60;
    c.advanced.memory_limit_mb = 256;
    c
}

fuzz_target!(|data: &[u8]| {
    // We need at least 9 bytes to make a meaningful packet:
    //   8 bytes for PTS (i64) + 1 byte for is_keyframe + data for payload
    if data.len() < 9 {
        return;
    }

    // Parse first 8 bytes as PTS (little-endian i64)
    let pts_bytes: [u8; 8] = data[..8].try_into().unwrap();
    let pts = i64::from_le_bytes(pts_bytes);

    // 9th byte: keyframe flag (0 = false, non-zero = true)
    let is_keyframe = data[8] != 0;

    // Remaining bytes: packet payload
    let payload = &data[9..];

    let packet = EncodedPacket {
        data: Bytes::copy_from_slice(payload),
        pts,
        dts: pts,
        is_keyframe,
        stream: StreamType::Video,
        resolution: None,
        codec: None,
    };

    // Push into a fresh buffer for maximum invariant exploration
    let config = fuzz_config();
    if let Ok(buffer) = LockFreeReplayBuffer::new(&config) {
        buffer.push(packet);

        // Snapshot — must never panic
        if let Ok(snapshot) = buffer.snapshot() {
            // Verify basic invariants
            for pair in snapshot.windows(2) {
                // PTS must be non-decreasing
                assert!(pair[0].pts <= pair[1].pts,
                    "Fuzz invariant violated: PTS {} > {}",
                    pair[0].pts, pair[1].pts,
                );
            }

            // All packets must have accessible data
            for packet in snapshot.iter() {
                let _len = packet.data.len();
                let _pts = packet.pts;
            }
        }
    }
});
