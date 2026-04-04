//! Integration: Config validation tests using rstest for parameterization.
//!
//! Demonstrates the use of rstest to reduce repetitive test code
//! for config validation edge cases.

mod common;

use liteclip_core::config::{Config, EncoderType, QualityPreset, RateControl, Resolution};
use rstest::rstest;

#[rstest]
#[case(0, 20, "zero bitrate clamped to minimum")]
#[case(1000, 500, "excessive bitrate clamped to maximum")]
#[case(50, 50, "valid bitrate unchanged")]
#[case(1, 1, "non-zero low bitrate unchanged")]
#[case(500, 500, "at maximum unchanged")]
fn config_validation_clamps_bitrate(
    #[case] input: u32,
    #[case] expected: u32,
    #[case] _label: &str,
) {
    let mut config = Config::default();
    config.video.bitrate_mbps = input;
    config.validate();
    assert_eq!(config.video.bitrate_mbps, expected);
}

#[rstest]
#[case(0, 30)]
#[case(1000, 240)]
#[case(60, 60)]
#[case(240, 240)]
#[case(15, 15)]
fn config_validation_clamps_framerate(#[case] input: u32, #[case] expected: u32) {
    let mut config = Config::default();
    config.video.framerate = input;
    config.validate();
    assert_eq!(config.video.framerate, expected);
}

#[rstest]
#[case(0, 30)]
#[case(1, 1)]
fn config_validation_clamps_replay_duration(#[case] input: u32, #[case] expected: u32) {
    let mut config = Config::default();
    config.general.replay_duration_secs = input;
    config.validate();
    assert_eq!(config.general.replay_duration_secs, expected);
}

#[rstest]
#[case(0, 1)]
#[case(1, 1)]
#[case(5, 5)]
fn config_validation_clamps_keyframe_interval(#[case] input: u32, #[case] expected: u32) {
    let mut config = Config::default();
    config.advanced.keyframe_interval_secs = input;
    config.validate();
    assert_eq!(config.advanced.keyframe_interval_secs, expected);
}

#[rstest]
#[case(EncoderType::Auto)]
#[case(EncoderType::Nvenc)]
#[case(EncoderType::Amf)]
#[case(EncoderType::Qsv)]
#[case(EncoderType::Software)]
fn encoder_type_serde_roundtrip(#[case] encoder: EncoderType) {
    let mut config = Config::default();
    config.video.encoder = encoder;
    let serialized = toml::to_string(&config).unwrap();
    let deserialized: Config = toml::from_str(&serialized).unwrap();
    assert_eq!(deserialized.video.encoder, encoder);
}

#[rstest]
#[case(QualityPreset::Performance)]
#[case(QualityPreset::Balanced)]
#[case(QualityPreset::Quality)]
fn quality_preset_serde_roundtrip(#[case] preset: QualityPreset) {
    let mut config = Config::default();
    config.video.quality_preset = preset;
    let serialized = toml::to_string(&config).unwrap();
    let deserialized: Config = toml::from_str(&serialized).unwrap();
    assert_eq!(deserialized.video.quality_preset, preset);
}

#[rstest]
#[case(Resolution::Native)]
#[case(Resolution::P480)]
#[case(Resolution::P720)]
#[case(Resolution::P1080)]
#[case(Resolution::P1440)]
#[case(Resolution::P2160)]
#[case(Resolution::UW1080)]
#[case(Resolution::UW1440)]
#[case(Resolution::UW2160)]
#[case(Resolution::SuperUW)]
#[case(Resolution::SuperUW1440)]
fn resolution_serde_roundtrip(#[case] res: Resolution) {
    let mut config = Config::default();
    config.video.resolution = res;
    config.video.use_native_resolution = res != Resolution::Native;
    let serialized = toml::to_string(&config).unwrap();
    let deserialized: Config = toml::from_str(&serialized).unwrap();
    assert_eq!(deserialized.video.resolution, res);
}

#[rstest]
#[case(Resolution::Custom(1920, 1080))]
#[case(Resolution::Custom(2560, 1440))]
#[case(Resolution::Custom(3840, 2160))]
#[case(Resolution::Custom(160, 160))]
#[case(Resolution::Custom(8192, 8192))]
fn custom_resolution_serde_roundtrip(#[case] res: Resolution) {
    let mut config = Config::default();
    config.video.resolution = res;
    config.video.use_native_resolution = false;
    let serialized = toml::to_string(&config).unwrap();
    let deserialized: Config = toml::from_str(&serialized).unwrap();
    assert_eq!(deserialized.video.resolution, res);
}

#[rstest]
#[case(RateControl::Cbr)]
#[case(RateControl::Vbr)]
#[case(RateControl::Cq)]
fn rate_control_serde_roundtrip(#[case] rc: RateControl) {
    let mut config = Config::default();
    config.video.rate_control = rc;
    let serialized = toml::to_string(&config).unwrap();
    let deserialized: Config = toml::from_str(&serialized).unwrap();
    assert_eq!(deserialized.video.rate_control, rc);
}

#[rstest]
#[case(-128i8, -100)] // i8 min -> clamped to -100
#[case(-101i8, -100)] // just outside valid range -> clamped
#[case(0i8, 0)]
#[case(100i8, 100)]
#[case(101i8, 100)] // just outside valid range -> clamped
#[case(127i8, 100)] // i8 max -> clamped to 100
fn config_validation_clamps_balance(#[case] input: i8, #[case] expected: i8) {
    let mut config = Config::default();
    config.audio.balance = input;
    config.validate();
    assert_eq!(config.audio.balance, expected);
}

#[rstest]
#[case(0, 0)]
#[case(100, 100)]
#[case(200, 200)]
#[case(255, 200)]
fn config_validation_clamps_master_volume(#[case] input: u8, #[case] expected: u8) {
    let mut config = Config::default();
    config.audio.master_volume = input;
    config.validate();
    assert_eq!(config.audio.master_volume, expected);
}

#[rstest]
#[case(0, 0)]
#[case(100, 100)]
#[case(200, 200)]
#[case(255, 200)]
fn config_validation_clamps_system_volume(#[case] input: u8, #[case] expected: u8) {
    let mut config = Config::default();
    config.audio.system_volume = input;
    config.validate();
    assert_eq!(config.audio.system_volume, expected);
}

#[rstest]
#[case(0, 0)]
#[case(200, 200)]
#[case(400, 400)]
#[case(500, 400)]
fn config_validation_clamps_mic_volume(#[case] input: u16, #[case] expected: u16) {
    let mut config = Config::default();
    config.audio.mic_volume = input;
    config.validate();
    assert_eq!(config.audio.mic_volume, expected);
}
