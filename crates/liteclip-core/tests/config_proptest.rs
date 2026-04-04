//! Property-based tests for config serialization.
//!
//! Uses proptest to verify that config roundtrips correctly with arbitrary valid values,
//! and that validation correctly handles edge cases and extreme values.
//! These tests provide stronger guarantees than example-based tests by exploring
//! a wide range of input combinations.

use liteclip_core::config::{Config, EncoderType, QualityPreset, Resolution};
use proptest::prelude::*;

/// Strategy for generating EncoderType values.
///
/// Produces all valid encoder type variants for comprehensive coverage.
fn arb_encoder() -> impl Strategy<Value = EncoderType> {
    prop_oneof![
        Just(EncoderType::Auto),
        Just(EncoderType::Nvenc),
        Just(EncoderType::Amf),
        Just(EncoderType::Qsv),
        Just(EncoderType::Software),
    ]
}

/// Strategy for generating QualityPreset values.
///
/// Produces all valid quality preset variants.
fn arb_quality() -> impl Strategy<Value = QualityPreset> {
    prop_oneof![
        Just(QualityPreset::Performance),
        Just(QualityPreset::Balanced),
        Just(QualityPreset::Quality),
    ]
}

/// Strategy for generating Resolution values.
///
/// Produces all valid resolution variants including Native.
fn arb_resolution() -> impl Strategy<Value = Resolution> {
    prop_oneof![
        Just(Resolution::Native),
        Just(Resolution::P480),
        Just(Resolution::P720),
        Just(Resolution::P1080),
        Just(Resolution::P1440),
        Just(Resolution::P2160),
    ]
}

/// Strategy for generating valid bitrate values in Mbps.
///
/// Range: 20-500 Mbps (reasonable recording bitrates)
fn arb_bitrate() -> impl Strategy<Value = u32> {
    20u32..=500
}

/// Strategy for generating valid framerate values.
///
/// Range: 1-240 FPS (common recording framerates)
fn arb_framerate() -> impl Strategy<Value = u32> {
    1u32..=240
}

/// Strategy for generating valid replay duration values.
///
/// Range: 10-300 seconds (10 seconds to 5 minutes)
fn arb_duration() -> impl Strategy<Value = u32> {
    10u32..300
}

/// Strategy for generating valid memory limit values.
///
/// Range: 128-2048 MB (128 MB to 2 GB)
fn arb_memory_limit() -> impl Strategy<Value = u32> {
    128u32..2048
}

/// Strategy for generating valid hotkey strings.
///
/// Produces common hotkey patterns used in the application.
fn arb_hotkey() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("Alt+F1".to_string()),
        Just("Alt+F2".to_string()),
        Just("Ctrl+Shift+F9".to_string()),
        Just("Ctrl+F5".to_string()),
        Just("Pause".to_string()),
    ]
}

/// Strategy for generating arbitrary valid Config instances.
///
/// Combines individual field strategies to produce complete configs.
fn arb_config() -> impl Strategy<Value = Config> {
    (
        arb_duration(),
        arb_encoder(),
        arb_framerate(),
        arb_bitrate(),
        arb_quality(),
        arb_resolution(),
        arb_memory_limit(),
        arb_hotkey(),
        arb_hotkey(),
    )
        .prop_map(
            |(
                duration,
                encoder,
                framerate,
                bitrate,
                quality,
                resolution,
                memory_limit,
                hotkey_save,
                hotkey_toggle,
            )| {
                let mut config = Config::default();
                config.general.replay_duration_secs = duration;
                config.video.encoder = encoder;
                config.video.framerate = framerate;
                config.video.bitrate_mbps = bitrate;
                config.video.quality_preset = quality;
                config.video.resolution = resolution;
                config.advanced.memory_limit_mb = memory_limit;
                config.hotkeys.save_clip = hotkey_save;
                config.hotkeys.toggle_recording = hotkey_toggle;
                config
            },
        )
}

// Property: Config should roundtrip through serialization unchanged.
//
// Verifies that any valid config can be serialized to TOML and
// deserialized back to an equivalent config. This is the fundamental
// property that ensures user settings are preserved.
proptest! {
    #[test]
    fn config_roundtrip_preserves_fields(config in arb_config()) {
        let serialized = toml::to_string(&config).unwrap();
        let deserialized: Config = toml::from_str(&serialized).unwrap();

        prop_assert_eq!(deserialized.general.replay_duration_secs, config.general.replay_duration_secs);
        prop_assert_eq!(deserialized.video.encoder, config.video.encoder);
        prop_assert_eq!(deserialized.video.framerate, config.video.framerate);
        prop_assert_eq!(deserialized.video.bitrate_mbps, config.video.bitrate_mbps);
        prop_assert_eq!(deserialized.video.quality_preset, config.video.quality_preset);
        prop_assert_eq!(deserialized.video.resolution, config.video.resolution);
        prop_assert_eq!(deserialized.advanced.memory_limit_mb, config.advanced.memory_limit_mb);
        prop_assert_eq!(deserialized.hotkeys.save_clip, config.hotkeys.save_clip);
        prop_assert_eq!(deserialized.hotkeys.toggle_recording, config.hotkeys.toggle_recording);
    }
}

// Property: Config validation should clamp invalid bitrate values.
//
// Zero bitrate is invalid and should be clamped to the minimum.
proptest! {
    #[test]
    fn config_validation_clamps_bitrate(
        bad_bitrate in prop::strategy::Just(0u32),
    ) {
        let mut config = Config::default();
        config.video.bitrate_mbps = bad_bitrate;
        config.validate();

        prop_assert_eq!(config.video.bitrate_mbps, 20);
    }
}

// Property: Config with extreme values should be clamped to valid ranges.
//
// Verifies that validation handles out-of-range values gracefully.
proptest! {
    #[test]
    fn config_clamps_extreme_values(
        extreme_bitrate in 0u32..2000,
        extreme_framerate in 0u32..500,
    ) {
        let mut config = Config::default();
        config.video.bitrate_mbps = extreme_bitrate;
        config.video.framerate = extreme_framerate;
        config.validate();

        // After validation, values should be in valid ranges
        // Bitrate: 0 -> 20, >500 -> 500, otherwise unchanged
        if extreme_bitrate == 0 {
            prop_assert_eq!(config.video.bitrate_mbps, 20);
        } else if extreme_bitrate > 500 {
            prop_assert_eq!(config.video.bitrate_mbps, 500);
        } else {
            prop_assert_eq!(config.video.bitrate_mbps, extreme_bitrate);
        }
        // Framerate: 0 -> 30, >240 -> 240, otherwise unchanged
        if extreme_framerate == 0 {
            prop_assert_eq!(config.video.framerate, 30);
        } else if extreme_framerate > 240 {
            prop_assert_eq!(config.video.framerate, 240);
        } else {
            prop_assert_eq!(config.video.framerate, extreme_framerate);
        }
    }
}

/// Property: Default config should serialize and deserialize correctly.
///
/// The default config is a special case that should always work.
#[test]
fn default_config_roundtrip() {
    let config = Config::default();
    let serialized = toml::to_string(&config).unwrap();
    let deserialized: Config = toml::from_str(&serialized).unwrap();

    assert_eq!(
        deserialized.general.replay_duration_secs,
        config.general.replay_duration_secs
    );
    assert_eq!(deserialized.video.framerate, config.video.framerate);
    assert_eq!(deserialized.video.bitrate_mbps, config.video.bitrate_mbps);
    assert_eq!(deserialized.video.encoder, config.video.encoder);
}

// Property: Config enums should roundtrip correctly through TOML.
//
// Verifies that enum serialization uses the correct string representations.
proptest! {
    #[test]
    fn encoder_type_roundtrip(encoder in arb_encoder()) {
        let mut config = Config::default();
        config.video.encoder = encoder;
        let serialized = toml::to_string(&config).unwrap();
        let deserialized: Config = toml::from_str(&serialized).unwrap();
        prop_assert_eq!(deserialized.video.encoder, encoder);
    }

    #[test]
    fn quality_preset_roundtrip(quality in arb_quality()) {
        let mut config = Config::default();
        config.video.quality_preset = quality;
        let serialized = toml::to_string(&config).unwrap();
        let deserialized: Config = toml::from_str(&serialized).unwrap();
        prop_assert_eq!(deserialized.video.quality_preset, quality);
    }

    #[test]
    fn resolution_roundtrip(resolution in arb_resolution()) {
        let mut config = Config::default();
        config.video.resolution = resolution;
        config.video.use_native_resolution = resolution != Resolution::Native;
        let serialized = toml::to_string(&config).unwrap();
        let deserialized: Config = toml::from_str(&serialized).unwrap();
        prop_assert_eq!(deserialized.video.resolution, resolution);
    }
}

/// Property: Config partial fields should deserialize with defaults.
///
/// Verifies that TOML with only some fields sets defaults for missing fields.
#[test]
fn config_partial_deserializes_with_defaults() {
    let partial_toml = r#"
[general]
replay_duration_secs = 60

[video]
framerate = 60
"#;

    let config: Config = toml::from_str(partial_toml).unwrap();

    // Explicit fields should be set
    assert_eq!(config.general.replay_duration_secs, 60);
    assert_eq!(config.video.framerate, 60);

    // Missing fields should use defaults
    assert_eq!(config.video.encoder, EncoderType::Auto);
    assert_eq!(config.video.quality_preset, QualityPreset::Performance);
}
