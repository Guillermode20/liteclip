//! E2E test utilities and shared harness for LiteClip application tests.
//!
//! This module provides a test harness for end-to-end testing of the full
//! application lifecycle, including app initialization, recording, clip saving,
//! and graceful shutdown.

#![allow(dead_code)]

pub mod app_harness;
pub mod event_simulator;
pub mod output_verifier;

use std::path::Path;
use std::time::Duration;

/// Default timeout for e2e test operations.
pub const DEFAULT_TEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Timeout for short operations like config changes.
pub const SHORT_TIMEOUT: Duration = Duration::from_secs(5);

/// Timeout for long operations like clip saving.
pub const LONG_TIMEOUT: Duration = Duration::from_secs(60);

/// Test configuration values optimized for fast e2e tests.
pub mod test_defaults {
    use liteclip::config::{Config, EncoderType, QualityPreset, Resolution};

    /// Creates a minimal config for fast e2e tests.
    ///
    /// Uses conservative settings to keep tests quick:
    /// - 10 second replay duration
    /// - 128 MB memory limit
    /// - 30 fps
    /// - 20 Mbps bitrate
    /// - Software encoder (no hardware dependency)
    /// - 720p resolution
    pub fn fast_test_config() -> Config {
        let mut config = Config::default();
        config.general.replay_duration_secs = 10;
        config.general.save_directory = std::env::temp_dir()
            .join("liteclip_e2e")
            .to_string_lossy()
            .to_string();
        config.video.framerate = 30;
        config.video.bitrate_mbps = 20;
        config.video.encoder = EncoderType::Software;
        config.video.quality_preset = QualityPreset::Balanced;
        config.video.resolution = Resolution::P720;
        config.advanced.memory_limit_mb = 128;
        config.general.start_minimised = true;
        config.general.auto_start_with_windows = false;
        config.general.auto_detect_game = false;
        config
    }

    /// Creates a config for testing high-quality paths.
    pub fn quality_test_config() -> Config {
        let mut config = fast_test_config();
        config.video.quality_preset = QualityPreset::Quality;
        config.video.bitrate_mbps = 50;
        config.video.resolution = Resolution::P1080;
        config
    }
}

/// Asserts that a path exists and is a file.
pub fn assert_file_exists(path: &Path) {
    assert!(path.exists(), "Expected file to exist: {}", path.display());
    assert!(
        path.is_file(),
        "Expected path to be a file: {}",
        path.display()
    );
}

/// Asserts that a file has a non-zero size.
pub fn assert_file_not_empty(path: &Path) {
    let metadata = std::fs::metadata(path)
        .unwrap_or_else(|e| panic!("Failed to read metadata for {}: {}", path.display(), e));
    assert!(
        metadata.len() > 0,
        "Expected file to have content: {}",
        path.display()
    );
}

/// Cleans up the liteclip temp directory used by tests.
pub fn cleanup_test_temp_dir() {
    let temp_dir = std::env::temp_dir().join("liteclip_e2e");
    if temp_dir.exists() {
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
