//! Integration: Config persistence and roundtrip tests.
//!
//! Tests configuration serialization to TOML and deserialization from disk.
//! These tests verify that user settings are correctly persisted and restored
//! across application sessions without data loss.

mod common;

use common::builders::ConfigBuilder;
use liteclip_core::config::Config;
use liteclip_core::paths::AppDirs;
use tempfile::TempDir;

/// Test: Config can be saved and loaded through custom directory paths.
///
/// Verifies the AppDirs abstraction correctly handles config file I/O
/// in a temporary directory without affecting user config.
#[test]
fn config_roundtrip_through_custom_dirs() -> anyhow::Result<()> {
    let temp_dir = TempDir::new()?;
    let config_file = temp_dir.path().join("settings.toml");
    let dirs = AppDirs::with_config_file(config_file, "test-clips")?;

    let mut cfg = Config::load_sync_from_dirs(&dirs)?;
    let original_duration = cfg.general.replay_duration_secs;
    cfg.general.replay_duration_secs = original_duration.max(45);
    cfg.save_sync_to_dirs(&dirs)?;

    let cfg2 = Config::load_sync_from_dirs(&dirs)?;
    assert_eq!(
        cfg2.general.replay_duration_secs,
        cfg.general.replay_duration_secs
    );

    Ok(())
}

/// Test: All config fields are preserved through save/load cycle.
///
/// Uses ConfigBuilder to create a config with specific values and verifies
/// each field is correctly serialized to TOML and deserialized back.
#[test]
fn config_roundtrip_preserves_all_fields() -> anyhow::Result<()> {
    let temp_dir = TempDir::new()?;
    let config_file = temp_dir.path().join("settings.toml");
    let dirs = AppDirs::with_config_file(config_file, "test-clips")?;

    let cfg = ConfigBuilder::new()
        .with_replay_duration(60)
        .with_framerate(120)
        .with_bitrate(50)
        .with_memory_limit(768)
        .build();

    cfg.save_sync_to_dirs(&dirs)?;
    let cfg2 = Config::load_sync_from_dirs(&dirs)?;

    assert_eq!(
        cfg2.general.replay_duration_secs, 60,
        "Replay duration should be preserved"
    );
    assert_eq!(cfg2.video.framerate, 120, "Framerate should be preserved");
    assert_eq!(cfg2.video.bitrate_mbps, 50, "Bitrate should be preserved");
    assert_eq!(
        cfg2.advanced.memory_limit_mb, 768,
        "Memory limit should be preserved"
    );

    Ok(())
}

/// Test: Config file is created if it doesn't exist.
///
/// Verifies that loading from a non-existent config file creates
/// a default configuration and the file is written on save.
#[test]
fn config_creates_file_when_missing() -> anyhow::Result<()> {
    let temp_dir = TempDir::new()?;
    let config_file = temp_dir.path().join("nonexistent.toml");
    let dirs = AppDirs::with_config_file(config_file.clone(), "test-clips")?;

    // File should not exist initially
    assert!(
        !config_file.exists(),
        "Config file should not exist initially"
    );

    // Load should create default config
    let cfg = Config::load_sync_from_dirs(&dirs)?;
    assert_eq!(
        cfg.general.replay_duration_secs, 30,
        "Should use default duration"
    );

    // Save should create the file
    cfg.save_sync_to_dirs(&dirs)?;
    assert!(
        config_file.exists(),
        "Config file should be created after save"
    );

    Ok(())
}
