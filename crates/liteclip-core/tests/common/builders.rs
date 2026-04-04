//! Builder patterns for constructing test configurations.
//!
//! This module provides a fluent API for creating Config instances with
//! specific settings. Builders make tests more readable and maintainable by
//! clearly expressing the intent of each test configuration.

use liteclip_core::config::{Config, EncoderType, QualityPreset, Resolution};

/// Builder for creating test configs with a fluent API.
///
/// Provides a chainable interface for constructing Config instances
/// with custom settings. Each method returns self for chaining.
///
/// # Example
///
/// ```
/// let config = ConfigBuilder::new()
///     .with_replay_duration(60)
///     .with_framerate(120)
///     .with_encoder(EncoderType::Nvenc)
///     .build();
/// ```
pub struct ConfigBuilder {
    config: Config,
}

impl ConfigBuilder {
    /// Create a new config builder with default config values.
    pub fn new() -> Self {
        Self {
            config: Config::default(),
        }
    }

    /// Set replay duration in seconds.
    ///
    /// Controls how long the replay buffer retains video.
    pub fn with_replay_duration(mut self, secs: u32) -> Self {
        self.config.general.replay_duration_secs = secs;
        self
    }

    /// Set encoder type.
    ///
    /// # Arguments
    ///
    /// * `encoder` - One of Auto, Nvenc, Amf, Qsv, or Software
    pub fn with_encoder(mut self, encoder: EncoderType) -> Self {
        self.config.video.encoder = encoder;
        self
    }

    /// Set framerate in frames per second.
    ///
    /// Common values: 30, 60, 120, 144
    pub fn with_framerate(mut self, fps: u32) -> Self {
        self.config.video.framerate = fps;
        self
    }

    /// Set bitrate in megabits per second.
    ///
    /// # Arguments
    ///
    /// * `bitrate_mbps` - Bitrate in Mbps (e.g., 20 for 20 Mbps)
    pub fn with_bitrate(mut self, bitrate_mbps: u32) -> Self {
        self.config.video.bitrate_mbps = bitrate_mbps;
        self
    }

    /// Set memory limit in megabytes.
    ///
    /// Controls the maximum memory usage for the replay buffer.
    pub fn with_memory_limit(mut self, mb: u32) -> Self {
        self.config.advanced.memory_limit_mb = mb;
        self
    }

    /// Set quality preset.
    ///
    /// Affects the trade-off between encoding quality and performance.
    pub fn with_quality(mut self, preset: QualityPreset) -> Self {
        self.config.video.quality_preset = preset;
        self
    }

    /// Set output resolution.
    ///
    /// Use `Resolution::Native` to capture at display resolution,
    /// or a specific resolution like `Resolution::P1080`.
    pub fn with_resolution(mut self, res: Resolution) -> Self {
        self.config.video.resolution = res;
        self
    }

    /// Set save directory path.
    ///
    /// Directory where clips will be saved.
    pub fn with_save_dir(mut self, path: std::path::PathBuf) -> Self {
        self.config.general.save_directory = path.to_string_lossy().to_string();
        self
    }

    /// Build the final Config instance.
    ///
    /// Consumes the builder and returns the configured Config.
    pub fn build(self) -> Config {
        self.config
    }
}

impl Default for ConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Create a config optimized for fast testing.
///
/// Uses minimal resources to keep tests quick:
/// - 30 second replay duration
/// - 256 MB memory limit
/// - 30 fps
/// - 50 Mbps bitrate
/// - Auto encoder selection
///
/// # Example
///
/// ```
/// let config = fast_test_config();
/// // Suitable for unit tests that need a valid config
/// ```
pub fn fast_test_config() -> Config {
    ConfigBuilder::new()
        .with_replay_duration(30)
        .with_memory_limit(256)
        .with_framerate(30)
        .with_bitrate(50)
        .with_encoder(EncoderType::Auto)
        .build()
}

/// Create a config that simulates a high-end system.
///
/// Uses premium settings for testing performance-critical paths:
/// - 120 second replay duration
/// - 1024 MB memory limit
/// - 60 fps
/// - 80 Mbps bitrate
/// - Quality preset
/// - NVENC encoder
///
/// # Example
///
/// ```
/// let config = high_end_test_config();
/// // Suitable for testing memory pressure and performance
/// ```
pub fn high_end_test_config() -> Config {
    ConfigBuilder::new()
        .with_replay_duration(120)
        .with_memory_limit(1024)
        .with_framerate(60)
        .with_bitrate(80)
        .with_quality(QualityPreset::Quality)
        .with_encoder(EncoderType::Nvenc)
        .build()
}

/// Create a config that simulates a low-end system.
///
/// Uses conservative settings for testing resource-constrained scenarios:
/// - 15 second replay duration
/// - 128 MB memory limit
/// - 24 fps
/// - 20 Mbps bitrate
/// - Performance preset
/// - Software encoder
///
/// # Example
///
/// ```
/// let config = low_end_test_config();
/// // Suitable for testing memory pressure handling
/// ```
pub fn low_end_test_config() -> Config {
    ConfigBuilder::new()
        .with_replay_duration(15)
        .with_memory_limit(128)
        .with_framerate(24)
        .with_bitrate(20)
        .with_quality(QualityPreset::Performance)
        .with_encoder(EncoderType::Software)
        .build()
}
