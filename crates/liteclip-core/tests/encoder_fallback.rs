//! Integration: Encoder configuration tests.
//!
//! Tests encoder configuration structures and health monitoring.
//! Note: Actual encoder resolution and fallback logic is tested in
//! the inline tests within the encode module (see functions.rs).

mod common;

use common::builders::ConfigBuilder;
use liteclip_core::config::{EncoderType, QualityPreset, RateControl};
use liteclip_core::encode::encoder_mod::{ResolvedEncoderConfig, ResolvedEncoderType};

/// Test: Encoder configuration through builder preserves all settings.
#[test]
fn encoder_config_through_builder() {
    let config = ConfigBuilder::new()
        .with_encoder(EncoderType::Auto)
        .with_framerate(60)
        .with_bitrate(50)
        .build();

    assert_eq!(config.video.encoder, EncoderType::Auto);
    assert_eq!(config.video.framerate, 60);
    assert_eq!(config.video.bitrate_mbps, 50);
}

/// Test: Explicit encoder selections are preserved in config.
#[test]
fn explicit_encoder_selections_preserved() {
    let auto_config = ConfigBuilder::new().with_encoder(EncoderType::Auto).build();
    assert_eq!(auto_config.video.encoder, EncoderType::Auto);

    let software_config = ConfigBuilder::new()
        .with_encoder(EncoderType::Software)
        .build();
    assert_eq!(software_config.video.encoder, EncoderType::Software);

    let nvenc_config = ConfigBuilder::new()
        .with_encoder(EncoderType::Nvenc)
        .build();
    assert_eq!(nvenc_config.video.encoder, EncoderType::Nvenc);
}

/// Test: ResolvedEncoderConfig stores encoder parameters correctly.
#[test]
fn resolved_encoder_config_fields() {
    let config = ResolvedEncoderConfig {
        bitrate_mbps: 50,
        framerate: 60,
        resolution: (1920, 1080),
        use_native_resolution: false,
        encoder_type: ResolvedEncoderType::Software,
        quality_preset: QualityPreset::Quality,
        rate_control: RateControl::Cbr,
        quality_value: Some(23),
        keyframe_interval_secs: 2,
        use_cpu_readback: false,
        output_index: 0,
    };

    assert_eq!(config.bitrate_mbps, 50);
    assert_eq!(config.framerate, 60);
    assert_eq!(config.resolution, (1920, 1080));
    assert_eq!(config.keyframe_interval_secs, 2);
}

/// Test: Encoder health event structure.
#[test]
fn encoder_health_event_creation() {
    use liteclip_core::encode::EncoderHealthEvent;

    let error_msg = "NVENC initialization failed".to_string();
    let event = EncoderHealthEvent::Fatal(error_msg.clone());

    match event {
        EncoderHealthEvent::Fatal(msg) => assert_eq!(msg, error_msg),
    }
}

/// Test: Quality preset enum variants.
#[test]
fn quality_preset_variants() {
    assert_ne!(QualityPreset::Performance, QualityPreset::Quality);
    assert_ne!(QualityPreset::Performance, QualityPreset::Balanced);
}

/// Test: Rate control enum variants.
#[test]
fn rate_control_variants() {
    assert_ne!(RateControl::Cbr, RateControl::Vbr);
}

/// Test: Encoder type enum variants and comparisons.
#[test]
fn encoder_type_variants() {
    assert_ne!(EncoderType::Auto, EncoderType::Software);
    assert_ne!(EncoderType::Nvenc, EncoderType::Amf);
    assert_eq!(EncoderType::Software, EncoderType::Software);
}

/// Test: Resolved encoder type variants.
#[test]
fn resolved_encoder_type_variants() {
    assert_ne!(ResolvedEncoderType::Software, ResolvedEncoderType::Nvenc);
    assert_ne!(ResolvedEncoderType::Nvenc, ResolvedEncoderType::Amf);
}

/// Test: Config validation handles edge cases.
#[test]
fn config_edge_cases() {
    let zero_bitrate = ResolvedEncoderConfig {
        bitrate_mbps: 0,
        framerate: 30,
        resolution: (1920, 1080),
        use_native_resolution: false,
        encoder_type: ResolvedEncoderType::Software,
        quality_preset: QualityPreset::Balanced,
        rate_control: RateControl::Cbr,
        quality_value: None,
        keyframe_interval_secs: 2,
        use_cpu_readback: false,
        output_index: 0,
    };
    assert_eq!(zero_bitrate.bitrate_mbps, 0);

    let zero_fps = ResolvedEncoderConfig {
        bitrate_mbps: 20,
        framerate: 0,
        resolution: (1920, 1080),
        use_native_resolution: false,
        encoder_type: ResolvedEncoderType::Software,
        quality_preset: QualityPreset::Balanced,
        rate_control: RateControl::Cbr,
        quality_value: None,
        keyframe_interval_secs: 2,
        use_cpu_readback: false,
        output_index: 0,
    };
    assert_eq!(zero_fps.framerate, 0);
}

/// Test: Encoder config with quality value (CQ/CRF).
#[test]
fn config_with_quality_value() {
    let config = ResolvedEncoderConfig {
        bitrate_mbps: 0,
        framerate: 30,
        resolution: (1920, 1080),
        use_native_resolution: false,
        encoder_type: ResolvedEncoderType::Software,
        quality_preset: QualityPreset::Quality,
        rate_control: RateControl::Cbr,
        quality_value: Some(18),
        keyframe_interval_secs: 2,
        use_cpu_readback: false,
        output_index: 0,
    };

    assert_eq!(config.quality_value, Some(18));
}

/// Test: CPU readback flag for hardware encoder fallback path.
#[test]
fn cpu_readback_flag() {
    let with_readback = ResolvedEncoderConfig {
        bitrate_mbps: 20,
        framerate: 30,
        resolution: (1920, 1080),
        use_native_resolution: false,
        encoder_type: ResolvedEncoderType::Software,
        quality_preset: QualityPreset::Balanced,
        rate_control: RateControl::Cbr,
        quality_value: None,
        keyframe_interval_secs: 2,
        use_cpu_readback: true,
        output_index: 0,
    };

    assert!(with_readback.use_cpu_readback);
}
