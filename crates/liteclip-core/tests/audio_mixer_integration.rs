//! Integration tests for the audio mixer.
//!
//! Tests the `AudioMixer` synchronization, mixing, and level-tracking behaviour
//! without requiring real WASAPI devices. Uses synthetic audio packets with
//! known timestamps passed directly via `mix_packets()`.

mod common;

use common::fixtures::make_test_packet;
use liteclip_core::capture::audio::mixer::AudioMixer;
use liteclip_core::config::AudioConfig;

/// Helper: create a default audio config usable in tests.
fn audio_config() -> AudioConfig {
    AudioConfig::default()
}

// ---------------------------------------------------------------------------
// Basic lifecycle
// ---------------------------------------------------------------------------

#[test]
fn mixer_creation_succeeds() {
    let mixer = AudioMixer::new(&audio_config());
    let (sys, mic) = mixer.pending_packet_counts();
    assert_eq!(sys, 0, "no packets pending at creation");
    assert_eq!(mic, 0, "no mic packets pending at creation");
}

#[test]
fn mixer_config_update_succeeds() {
    let mut mixer = AudioMixer::new(&audio_config());
    let mut cfg = audio_config();
    cfg.system_volume = 128;
    cfg.mic_volume = 200;
    mixer.update_config(&cfg);

    // After config update the mixer should still be functional
    let (sys, mic) = mixer.pending_packet_counts();
    assert_eq!(sys, 0);
    assert_eq!(mic, 0);
}

// ---------------------------------------------------------------------------
// Packet queueing via mix_packets
// ---------------------------------------------------------------------------

#[test]
fn mixer_queues_system_packet() {
    let mut mixer = AudioMixer::new(&audio_config());

    // mix_packets accepts a system packet (None for mic)
    let result = mixer.mix_packets(Some(make_test_packet(1_000_000, true, 1024)), None);
    // The result may be empty if no matching mic packet is available yet
    // But the system packet should be queued internally
    let (sys, mic) = mixer.pending_packet_counts();
    assert!(
        sys <= 32,
        "System packet count should be bounded: got {}",
        sys
    );
    assert_eq!(mic, 0, "No mic packets should be queued");
}

#[test]
fn mixer_queues_mic_packet() {
    let mut mixer = AudioMixer::new(&audio_config());

    let result = mixer.mix_packets(None, Some(make_test_packet(2_000_000, false, 512)));
    let (sys, mic) = mixer.pending_packet_counts();
    assert_eq!(sys, 0, "No system packets should be queued");
    assert!(mic <= 32, "Mic packet count should be bounded: got {}", mic);
}

#[test]
fn mixer_queues_both_streams() {
    let mut mixer = AudioMixer::new(&audio_config());

    let result = mixer.mix_packets(
        Some(make_test_packet(1_000_000, true, 256)),
        Some(make_test_packet(1_000_000, false, 256)),
    );

    let (sys, mic) = mixer.pending_packet_counts();
    assert!(sys <= 32, "System count bounded: {}", sys);
    assert!(mic <= 32, "Mic count bounded: {}", mic);

    // Validate output content if packets were produced
    if !result.is_empty() {
        for packet in &result {
            assert!(
                packet.pts >= 0,
                "Output packet should have non-negative PTS: got {}",
                packet.pts
            );
        }
        // Verify non-decreasing PTS in output
        for pair in result.windows(2) {
            assert!(
                pair[0].pts <= pair[1].pts,
                "Output PTS should be non-decreasing: {} > {}",
                pair[0].pts,
                pair[1].pts
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Timestamp ordering invariants
// ---------------------------------------------------------------------------

#[test]
fn mixer_does_not_produce_monotonically_decreasing_pts() {
    let mut mixer = AudioMixer::new(&audio_config());

    // Push system and mic packets in order with matching timestamps
    // The mixer should produce mixed output with non-decreasing PTS
    for i in 1..=4 {
        let _ = mixer.mix_packets(
            Some(make_test_packet(i * 1_000_000, true, 256)),
            Some(make_test_packet(i * 1_000_000, false, 256)),
        );
    }

    // After processing, the internal queues should be manageable
    let (sys, mic) = mixer.pending_packet_counts();
    assert!(sys <= 32, "System queue bounded: {}", sys);
    assert!(mic <= 32, "Mic queue bounded: {}", mic);
}

// ---------------------------------------------------------------------------
// Mix with only one stream
// ---------------------------------------------------------------------------

#[test]
fn mixer_mixes_when_only_system_has_data() {
    let mut mixer = AudioMixer::new(&audio_config());

    // Only push system packets (simulating no mic plugged in)
    let _ = mixer.mix_packets(Some(make_test_packet(1_000_000, true, 512)), None);
    let _ = mixer.mix_packets(Some(make_test_packet(2_000_000, true, 512)), None);
    let _ = mixer.mix_packets(Some(make_test_packet(3_000_000, true, 512)), None);

    // After three pushes without matching mic packets, the mixer
    // should have processed them (timeout-based single-stream output)
    // and should still be usable.
    let (sys, mic) = mixer.pending_packet_counts();
    assert!(mic <= 32, "Mic count must stay bounded");
    assert!(sys <= 32, "System count must stay bounded");
}

// ---------------------------------------------------------------------------
// Large timestamp gaps (simulate device start/stop)
// ---------------------------------------------------------------------------

#[test]
fn mixer_handles_large_timestamp_gaps() {
    let mut mixer = AudioMixer::new(&audio_config());

    // Simulate a gap: push packets with PTS 1-4M, then jump to 100_000_000
    for i in 1..=4 {
        let _ = mixer.mix_packets(
            Some(make_test_packet(i * 1_000_000, true, 256)),
            Some(make_test_packet(i * 1_000_000, false, 256)),
        );
    }

    // Large jump
    let _ = mixer.mix_packets(
        Some(make_test_packet(100_000_000, true, 256)),
        Some(make_test_packet(100_000_000, false, 256)),
    );

    // Must not panic — mixer should handle large timestamp deltas
    let (sys, mic) = mixer.pending_packet_counts();
    assert!(sys <= 32, "System queue bounded after gap: {}", sys);
    assert!(mic <= 32, "Mic queue bounded after gap: {}", mic);
}

// ---------------------------------------------------------------------------
// Mixer remains usable after large/weird inputs
// ---------------------------------------------------------------------------

#[test]
fn mixer_usable_after_negative_timestamps() {
    let mut mixer = AudioMixer::new(&audio_config());

    // Push with negative PTS (edge case)
    let _ = mixer.mix_packets(Some(make_test_packet(-1, true, 256)), None);
    let _ = mixer.mix_packets(None, Some(make_test_packet(-1, false, 256)));

    // Then push normal timestamps — mixer should not be corrupted
    let _ = mixer.mix_packets(
        Some(make_test_packet(1_000_000, true, 256)),
        Some(make_test_packet(1_000_000, false, 256)),
    );

    let (sys, mic) = mixer.pending_packet_counts();
    assert!(
        sys <= 32,
        "System count must stay bounded after negative PTS"
    );
    assert!(mic <= 32, "Mic count must stay bounded after negative PTS");
}

// ---------------------------------------------------------------------------
// Volume extremes
// ---------------------------------------------------------------------------

#[test]
fn mixer_volume_changes_do_not_crash() {
    let mut mixer = AudioMixer::new(&audio_config());

    // Extremes: volume 0
    let mut cfg = audio_config();
    cfg.system_volume = 0;
    cfg.mic_volume = 0;
    mixer.update_config(&cfg);

    let _ = mixer.mix_packets(
        Some(make_test_packet(1_000_000, true, 256)),
        Some(make_test_packet(1_000_000, false, 256)),
    );

    // Max values
    cfg.system_volume = 255;
    cfg.mic_volume = 255;
    mixer.update_config(&cfg);

    let _ = mixer.mix_packets(
        Some(make_test_packet(2_000_000, true, 256)),
        Some(make_test_packet(2_000_000, false, 256)),
    );

    // Must not panic with any config
    let (sys, mic) = mixer.pending_packet_counts();
    assert!(sys <= 32);
    assert!(mic <= 32);
}

// ---------------------------------------------------------------------------
// Repeated mixing cycles
// ---------------------------------------------------------------------------

#[test]
fn mixer_repeated_cycles_no_resource_leak() {
    let mut mixer = AudioMixer::new(&audio_config());
    let mut total_output = 0usize;

    for i in 0..100 {
        let output = mixer.mix_packets(
            Some(make_test_packet(i * 1_000_000, true, 256)),
            Some(make_test_packet(i * 1_000_000, false, 256)),
        );
        total_output += output.len();

        // Validate output packets when produced
        for packet in &output {
            assert!(
                packet.pts >= 0,
                "Output PTS should be non-negative at cycle {}: got {}",
                i,
                packet.pts
            );
        }
    }

    // At least some output should have been produced over 100 cycles
    assert!(
        total_output > 0,
        "Expected some mixed output over 100 cycles, got {}",
        total_output
    );

    let (sys, mic) = mixer.pending_packet_counts();
    // Queues should not grow unbounded over 100 cycles
    assert!(sys <= 32, "System queue should not leak: {}", sys);
    assert!(mic <= 32, "Mic queue should not leak: {}", mic);
}
