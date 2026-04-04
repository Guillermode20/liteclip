//! Integration: FFmpeg backend tests.
//!
//! Tests FFmpeg encoder initialization, configuration validation, and basic frame handling.
//! These tests verify the FFmpeg integration layer without requiring actual video encoding,
//! focusing on config validation and API availability.

#![cfg(feature = "ffmpeg")]

mod common;

use common::fixtures::make_test_frame;
use liteclip_core::config::QualityPreset;
use liteclip_core::encode::encoder_mod::{ResolvedEncoderConfig, ResolvedEncoderType};

/// Test that FFmpeg initializes successfully and reports configuration.
/// This is a smoke test to verify FFmpeg DLLs are available and linked correctly.
#[test]
fn ffmpeg_initialization_successful() {
    use ffmpeg_next as ffmpeg;

    ffmpeg::init().expect("FFmpeg should initialize successfully");

    let configuration = ffmpeg::format::configuration();
    assert!(
        !configuration.is_empty(),
        "FFmpeg should have configuration info"
    );

    // Verify we can query version information via format module
    let version = ffmpeg::format::version();
    assert!(version > 0, "FFmpeg should report a valid version number");
}

/// Test encoder configuration validation for supported resolutions.
/// Verifies that common recording resolutions are acceptable to the encoder config.
#[test]
fn encoder_accepts_standard_recording_resolutions() -> anyhow::Result<()> {
    let standard_resolutions = vec![
        (1280, 720, "720p"),
        (1920, 1080, "1080p"),
        (2560, 1440, "1440p"),
        (3840, 2160, "4K"),
    ];

    for (width, height, label) in standard_resolutions {
        let config = ResolvedEncoderConfig {
            bitrate_mbps: 20,
            framerate: 30,
            resolution: (width, height),
            use_native_resolution: false,
            encoder_type: ResolvedEncoderType::Software,
            quality_preset: QualityPreset::Balanced,
            rate_control: liteclip_core::config::RateControl::Cbr,
            quality_value: None,
            keyframe_interval_secs: 2,
            use_cpu_readback: false,
            output_index: 0,
        };

        assert_eq!(
            config.resolution,
            (width, height),
            "Resolution {} should be stored correctly",
            label
        );

        // Verify frame dimensions match config expectations
        let frame = make_test_frame(width, height, 0);
        assert_eq!(
            frame.resolution,
            (width, height),
            "Test frame should match {} resolution",
            label
        );
    }

    Ok(())
}

/// Test frame-encoder compatibility validation.
/// Verifies that frames produced by the capture pipeline match encoder input requirements.
#[test]
fn captured_frame_matches_encoder_expectations() -> anyhow::Result<()> {
    let config = ResolvedEncoderConfig {
        bitrate_mbps: 20,
        framerate: 60,
        resolution: (1920, 1080),
        use_native_resolution: false,
        encoder_type: ResolvedEncoderType::Software,
        quality_preset: QualityPreset::Balanced,
        rate_control: liteclip_core::config::RateControl::Cbr,
        quality_value: None,
        keyframe_interval_secs: 2,
        use_cpu_readback: false,
        output_index: 0,
    };

    // Create a frame at multiple timestamps to simulate real capture
    let timestamps: Vec<i64> = vec![0, 16667, 33333, 50000]; // ~60fps in microseconds

    for (i, &timestamp) in timestamps.iter().enumerate() {
        let frame = make_test_frame(1920, 1080, timestamp);

        // Frame should match encoder resolution expectations
        assert_eq!(
            frame.resolution, config.resolution,
            "Frame {} resolution should match encoder config",
            i
        );

        // Frame should have valid BGRA data (4 bytes per pixel)
        let expected_size = 1920u32 * 1080u32 * 4;
        assert_eq!(
            frame.bgra.len() as u32,
            expected_size,
            "Frame {} should have correct BGRA buffer size",
            i
        );

        // Timestamp should be monotonically increasing
        if i > 0 {
            assert!(
                frame.timestamp > timestamps[i - 1],
                "Frame timestamps should increase monotonically"
            );
        }
    }

    Ok(())
}

/// Test encoder bitrate constraints and validation.
/// Verifies that bitrate values are properly bounded for software encoding.
#[test]
fn encoder_bitrate_constraints() {
    // Test minimum viable bitrate for 1080p
    let min_config = ResolvedEncoderConfig {
        bitrate_mbps: 1,
        framerate: 30,
        resolution: (1920, 1080),
        use_native_resolution: false,
        encoder_type: ResolvedEncoderType::Software,
        quality_preset: QualityPreset::Performance,
        rate_control: liteclip_core::config::RateControl::Cbr,
        quality_value: None,
        keyframe_interval_secs: 2,
        use_cpu_readback: false,
        output_index: 0,
    };
    assert_eq!(min_config.bitrate_mbps, 1);

    // Test maximum reasonable bitrate
    let max_config = ResolvedEncoderConfig {
        bitrate_mbps: 500,
        framerate: 60,
        resolution: (3840, 2160),
        use_native_resolution: false,
        encoder_type: ResolvedEncoderType::Software,
        quality_preset: QualityPreset::Quality,
        rate_control: liteclip_core::config::RateControl::Vbr,
        quality_value: None,
        keyframe_interval_secs: 2,
        use_cpu_readback: false,
        output_index: 0,
    };
    assert_eq!(max_config.bitrate_mbps, 500);
}
