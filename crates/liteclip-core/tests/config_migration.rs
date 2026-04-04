//! Integration: Config migration and version compatibility tests.
//!
//! Tests that config files from older versions can be loaded and migrated,
//! and that configs with unknown fields (future versions) can still be loaded.
//! These tests ensure backward and forward compatibility of user settings.

use liteclip_core::config::Config;
use tempfile::TempDir;

/// Test: Minimal config from older version loads successfully with defaults.
///
/// Verifies that a config file with only essential fields (as might be
/// written by an older version) loads correctly and uses default values
/// for any missing fields.
#[test]
fn load_minimal_config_from_older_version() -> anyhow::Result<()> {
    let temp_dir = TempDir::new()?;
    let config_file = temp_dir.path().join("liteclip.toml");

    // Simulate an older config file with fewer fields
    let old_config = r#"
[general]
replay_duration_secs = 60
save_directory = "C:\\clips"

[video]
encoder = "auto"
framerate = 60
bitrate_mbps = 20
"#;

    std::fs::write(&config_file, old_config)?;

    // Load should succeed even with missing fields (uses defaults)
    let content = std::fs::read_to_string(&config_file)?;
    let config: Config = toml::from_str(&content)?;

    // Verify loaded values
    assert_eq!(config.general.replay_duration_secs, 60);
    assert_eq!(config.video.framerate, 60);
    assert_eq!(config.video.bitrate_mbps, 20);

    // Verify defaults were applied for missing fields
    assert_eq!(
        config.video.quality_preset,
        liteclip_core::config::QualityPreset::Performance
    );

    Ok(())
}

/// Test: Config with unknown future fields loads successfully.
///
/// Verifies forward compatibility - if a user downgrades after using
/// a newer version with additional settings, the config still loads
/// and unknown fields are ignored.
#[test]
fn load_config_with_unknown_fields() -> anyhow::Result<()> {
    let temp_dir = TempDir::new()?;
    let config_file = temp_dir.path().join("liteclip.toml");

    // Config with future/unknown fields
    let config_with_unknown = r#"
[general]
replay_duration_secs = 120
save_directory = "C:\\clips"
unknown_future_field = "test"

[video]
encoder = "auto"
framerate = 60
bitrate_mbps = 50
experimental_feature = true

[unknown_section]
some_value = 123
"#;

    std::fs::write(&config_file, config_with_unknown)?;

    // Load should succeed, unknown fields are ignored
    let content = std::fs::read_to_string(&config_file)?;
    let config: Config = toml::from_str(&content)?;

    assert_eq!(config.general.replay_duration_secs, 120);
    assert_eq!(config.video.framerate, 60);

    Ok(())
}

/// Test: Full config roundtrip preserves all known fields.
///
/// Serializes a config with all fields set to specific values,
/// then deserializes and verifies every field is preserved exactly.
#[test]
fn config_serialization_roundtrip_all_fields() -> anyhow::Result<()> {
    let mut config = Config::default();
    config.general.replay_duration_secs = 90;
    config.general.save_directory = "C:\\test\\clips".to_string();
    config.video.framerate = 120;
    config.video.bitrate_mbps = 35;
    config.video.encoder = liteclip_core::config::EncoderType::Nvenc;
    config.video.quality_preset = liteclip_core::config::QualityPreset::Quality;
    config.advanced.memory_limit_mb = 768;

    // Serialize
    let serialized = toml::to_string(&config)?;

    // Deserialize
    let deserialized: Config = toml::from_str(&serialized)?;

    // Verify all fields match
    assert_eq!(deserialized.general.replay_duration_secs, 90);
    assert_eq!(deserialized.general.save_directory, "C:\\test\\clips");
    assert_eq!(deserialized.video.framerate, 120);
    assert_eq!(deserialized.video.bitrate_mbps, 35);
    assert_eq!(
        deserialized.video.encoder,
        liteclip_core::config::EncoderType::Nvenc
    );
    assert_eq!(
        deserialized.video.quality_preset,
        liteclip_core::config::QualityPreset::Quality
    );
    assert_eq!(deserialized.advanced.memory_limit_mb, 768);

    Ok(())
}

/// Test: Config validation clamps invalid bitrate values.
///
/// Verifies that bitrate values outside the valid range are
/// automatically adjusted to acceptable bounds.
#[test]
fn config_validation_clamps_bitrate() {
    let mut config = Config::default();
    config.video.bitrate_mbps = 0; // Invalid: too low
    config.validate();
    assert_eq!(
        config.video.bitrate_mbps, 20,
        "Zero bitrate should be clamped to minimum"
    );

    let mut config2 = Config::default();
    config2.video.bitrate_mbps = 1000; // Invalid: too high
    config2.validate();
    assert_eq!(
        config2.video.bitrate_mbps, 500,
        "Excessive bitrate should be clamped to maximum"
    );
}

/// Test: Config validation clamps invalid framerate values.
///
/// Verifies that framerate values outside the valid range are
/// automatically adjusted to acceptable bounds.
#[test]
fn config_validation_clamps_framerate() {
    let mut config = Config::default();
    config.video.framerate = 0; // Invalid: too low
    config.validate();
    assert_eq!(
        config.video.framerate, 30,
        "Zero framerate should be clamped to default"
    );

    let mut config2 = Config::default();
    config2.video.framerate = 1000; // Invalid: too high
    config2.validate();
    assert_eq!(
        config2.video.framerate, 240,
        "Excessive framerate should be clamped to maximum"
    );
}

/// Test: Hotkey parsing accepts valid hotkey formats.
///
/// Verifies that common hotkey patterns are accepted and preserved.
#[test]
fn hotkey_parsing_and_validation() {
    let mut config = Config::default();
    config.hotkeys.save_clip = "Alt+F1".to_string();
    config.validate();

    // Valid hotkey should remain unchanged
    assert_eq!(config.hotkeys.save_clip, "Alt+F1");

    // Test with another valid hotkey
    config.hotkeys.save_clip = "Ctrl+Shift+F9".to_string();
    config.validate();
    assert_eq!(config.hotkeys.save_clip, "Ctrl+Shift+F9");
}

/// Test: Default config with directories sets appropriate save location.
///
/// Verifies that when creating a default config with specific directories,
/// the save directory is set appropriately.
#[test]
fn config_default_with_dirs_sets_save_dir() -> anyhow::Result<()> {
    use liteclip_core::paths::AppDirs;

    let temp_dir = TempDir::new()?;
    let clips_dir = temp_dir.path().join("clips");
    std::fs::create_dir_all(&clips_dir)?;

    // This test is Windows-specific but should compile on all platforms
    #[cfg(windows)]
    {
        let dirs = AppDirs {
            config_dir: temp_dir.path().to_path_buf(),
            config_file: temp_dir.path().join("liteclip.toml"),
            clips_folder_name: "clips".to_string(),
        };

        let config = Config::default_with_dirs(&dirs);
        assert!(!config.general.save_directory.is_empty());
    }

    Ok(())
}

/// Test: Enum serialization roundtrips correctly through TOML.
///
/// Verifies that EncoderType, QualityPreset, and Resolution enums
/// serialize to their string representations and deserialize correctly.
#[test]
fn enum_serialization_roundtrip() -> anyhow::Result<()> {
    // Test EncoderType through config
    let mut config = Config::default();
    config.video.encoder = liteclip_core::config::EncoderType::Nvenc;
    let serialized = toml::to_string(&config)?;
    let deserialized: Config = toml::from_str(&serialized)?;
    assert_eq!(
        deserialized.video.encoder,
        liteclip_core::config::EncoderType::Nvenc
    );

    // Test QualityPreset through config
    let mut config2 = Config::default();
    config2.video.quality_preset = liteclip_core::config::QualityPreset::Performance;
    let serialized2 = toml::to_string(&config2)?;
    let deserialized2: Config = toml::from_str(&serialized2)?;
    assert_eq!(
        deserialized2.video.quality_preset,
        liteclip_core::config::QualityPreset::Performance
    );

    // Test Resolution through config
    let mut config3 = Config::default();
    config3.video.resolution = liteclip_core::config::Resolution::P1080;
    let serialized3 = toml::to_string(&config3)?;
    let deserialized3: Config = toml::from_str(&serialized3)?;
    assert_eq!(
        deserialized3.video.resolution,
        liteclip_core::config::Resolution::P1080
    );

    Ok(())
}
