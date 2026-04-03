//! Configuration types
//!
//! This module provides all configuration types for the application,
//! including video, audio, hotkeys, and general settings.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::paths::AppDirs;

use super::functions::{
    default_audio_normalization_enabled, default_audio_target_lufs, default_balance,
    default_bitrate, default_encoder, default_false, default_framerate, default_gpu_index,
    default_hotkey_gallery, default_hotkey_save, default_hotkey_screenshot, default_hotkey_toggle,
    default_keyframe_interval, default_master_volume, default_mic_device, default_mic_volume,
    default_quality_preset, default_quality_value, default_quality_value_for_preset,
    default_rate_control, default_replay_duration, default_resolution, default_save_directory,
    default_system_volume, default_true, default_true_peak_limit_dbtp,
    default_true_peak_limiter_enabled, ESTIMATED_MIC_AUDIO_BITRATE_BPS,
    ESTIMATED_SYSTEM_AUDIO_BITRATE_BPS, MAX_FRAMERATE, MAX_REPLAY_MEMORY_LIMIT_MB,
    MIN_REPLAY_MEMORY_LIMIT_MB, RECOMMENDED_BUFFER_BASE_OVERHEAD_MB,
    RECOMMENDED_BUFFER_HEADROOM_PERCENT, REPLAY_MEMORY_LIMIT_AUTO_MB,
};

/// Encoder selection for video encoding.
///
/// Specifies which encoder to use. Hardware encoders provide better performance
/// while software encoding works on any system.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EncoderType {
    /// Automatically select the best available encoder.
    ///
    /// Priority order: NVENC → AMF → QSV → Software (libx265).
    /// Software fallback ensures the app is usable on any system.
    Auto,
    /// NVIDIA NVENC (requires NVIDIA GPU).
    ///
    /// Encoding implementation: `src/encode/ffmpeg/nvenc.rs` (NVENC).
    Nvenc,
    /// AMD AMF (requires AMD GPU).
    Amf,
    /// Intel Quick Sync Video (requires Intel iGPU).
    ///
    /// Encoding implementation: `src/encode/ffmpeg/qsv.rs` (QSV).
    Qsv,
    /// Software encoder (libx265 HEVC, works on any CPU).
    ///
    /// Encoding implementation: `src/encode/ffmpeg/software.rs` (CPU path).
    /// This encoder uses CPU-only encoding with no GPU frame transport.
    Software,
}

/// High-level quality/speed tradeoff for encoder options.
///
/// Controls the encoder preset which affects encoding speed vs output quality.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QualityPreset {
    /// Maximum performance (fastest encoding, lower quality).
    Performance,
    /// Balanced between speed and quality.
    Balanced,
    /// Maximum quality (slowest encoding, best quality).
    Quality,
}

/// Resolution options for video capture.
///
/// Specifies the capture and output resolution.
/// Supports standard 16:9, ultrawide 21:9/32:9, and custom resolutions.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    /// Use the native/desktop resolution.
    Native,
    /// 1920x1080 resolution (Full HD, 16:9).
    #[serde(rename = "1080p")]
    P1080,
    /// 1280x720 resolution (HD, 16:9).
    #[serde(rename = "720p")]
    P720,
    /// 854x480 resolution (480p, 16:9).
    #[serde(rename = "480p")]
    P480,
    /// 2560x1440 resolution (QHD/2K, 16:9).
    #[serde(rename = "1440p")]
    P1440,
    /// 3840x2160 resolution (4K UHD, 16:9).
    #[serde(rename = "2160p")]
    P2160,
    /// 2560x1080 resolution (Ultrawide 21:9, 1080p height).
    #[serde(rename = "uw1080")]
    UW1080,
    /// 3440x1440 resolution (Ultrawide 21:9, 1440p height).
    #[serde(rename = "uw1440")]
    UW1440,
    /// 5120x2160 resolution (Ultrawide 21:9, 4K height).
    #[serde(rename = "uw2160")]
    UW2160,
    /// 3840x1080 resolution (Super Ultrawide 32:9).
    #[serde(rename = "superuw")]
    SuperUW,
    /// 5120x1440 resolution (Super Ultrawide 32:9, 1440p height).
    #[serde(rename = "superuw1440")]
    SuperUW1440,
    /// Custom resolution with user-defined width and height.
    /// Stored as (width, height) tuple. Must be divisible by 2 for encoder compatibility.
    #[serde(rename = "custom")]
    Custom(u32, u32),
}

/// Root configuration structure.
///
/// Contains all configuration sections for the application:
/// - General settings (replay buffer, save directory, etc.)
/// - Video settings (resolution, framerate, encoder, codec)
/// - Audio settings (capture sources, volume levels)
/// - Hotkey bindings
/// - Advanced settings
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    /// General application settings.
    #[serde(default)]
    pub general: GeneralConfig,
    /// Video encoding settings.
    #[serde(default)]
    pub video: VideoConfig,
    /// Audio capture settings.
    #[serde(default)]
    pub audio: AudioConfig,
    /// Hotkey bindings.
    #[serde(default)]
    pub hotkeys: HotkeyConfig,
    /// Advanced/developer settings.
    #[serde(default)]
    pub advanced: AdvancedConfig,
}

impl Config {
    /// Like [`Self::default`], but sets `general.save_directory` from [`AppDirs::default_save_directory_string`].
    pub fn default_with_dirs(dirs: &AppDirs) -> Self {
        let mut config = Self::default();
        config.general.save_directory = dirs.default_save_directory_string();
        config
    }

    /// Loads configuration from [`AppDirs::liteclip`].
    ///
    /// Configuration file: `%APPDATA%\liteclip\liteclip.toml`.
    /// If the file doesn't exist, creates defaults and writes the file.
    pub async fn load() -> Result<Self> {
        Self::load_with_dirs(&AppDirs::liteclip()?).await
    }

    /// Load using explicit application directories (embedders).
    ///
    /// If the config file is missing, builds [`Self::default_with_dirs`], validates, **writes**
    /// the file to disk, then returns it. For sync loading without persisting defaults, use
    /// [`Self::load_sync_from_dirs`].
    pub async fn load_with_dirs(dirs: &AppDirs) -> Result<Self> {
        let config_path = &dirs.config_file;
        if config_path.exists() {
            let content = tokio::fs::read_to_string(config_path)
                .await
                .with_context(|| format!("Failed to read config from {:?}", config_path))?;
            let mut config: Config = toml::from_str(&content)?;
            config.validate();
            Ok(config)
        } else {
            let mut config = Self::default_with_dirs(dirs);
            config.validate();
            config.save_to_dirs(dirs).await?;
            Ok(config)
        }
    }

    /// Save configuration next to [`AppDirs::liteclip`].
    pub async fn save(&self) -> Result<()> {
        self.save_to_dirs(&AppDirs::liteclip()?).await
    }

    /// Save to the TOML path implied by `dirs`.
    pub async fn save_to_dirs(&self, dirs: &AppDirs) -> Result<()> {
        let config_path = &dirs.config_file;
        let parent = config_path
            .parent()
            .context("Config path has no parent directory")?;
        tokio::fs::create_dir_all(parent).await?;
        let content = toml::to_string_pretty(self)?;
        tokio::fs::write(config_path, content)
            .await
            .with_context(|| format!("Failed to write config to {:?}", config_path))?;
        Ok(())
    }

    /// Save synchronously — used from the GUI thread which has no tokio runtime.
    pub fn save_sync(&self) -> Result<()> {
        self.save_sync_to_dirs(&AppDirs::liteclip()?)
    }

    /// Save synchronously using explicit [`AppDirs`].
    pub fn save_sync_to_dirs(&self, dirs: &AppDirs) -> Result<()> {
        let config_path = &dirs.config_file;
        let parent = config_path
            .parent()
            .context("Config path has no parent directory")?;
        std::fs::create_dir_all(parent).context("Failed to create config directory")?;
        let content = toml::to_string_pretty(self)?;
        std::fs::write(config_path, &content)
            .with_context(|| format!("Failed to write config to {:?}", config_path))?;
        Ok(())
    }

    /// Load synchronously — used from the GUI thread which has no tokio runtime.
    pub fn load_sync() -> Result<Self> {
        Self::load_sync_from_dirs(&AppDirs::liteclip()?)
    }

    /// Load synchronously from explicit [`AppDirs`].
    ///
    /// If the file is missing, returns [`Self::default_with_dirs`] **without** writing it
    /// (unlike [`Self::load_with_dirs`], which persists defaults when the file is absent).
    pub fn load_sync_from_dirs(dirs: &AppDirs) -> Result<Self> {
        let config_path = &dirs.config_file;
        if config_path.exists() {
            let content = std::fs::read_to_string(config_path)
                .with_context(|| format!("Failed to read config from {:?}", config_path))?;
            let mut config: Self = toml::from_str(&content)
                .with_context(|| format!("Failed to parse config from {:?}", config_path))?;
            config.validate();
            Ok(config)
        } else {
            Ok(Self::default_with_dirs(dirs))
        }
    }

    /// Configuration file path for LiteClip defaults.
    pub fn config_path() -> Result<PathBuf> {
        Ok(AppDirs::liteclip()?.config_file)
    }
    /// Validate and sanitize configuration values
    ///
    /// Clamps values to safe ranges to prevent panics from invalid user input.
    pub fn validate(&mut self) {
        use tracing::warn;
        if self.video.framerate == 0 {
            warn!("Config: framerate was 0, clamping to 30");
            self.video.framerate = 30;
        } else if self.video.framerate > MAX_FRAMERATE {
            warn!(
                "Config: framerate was {}, clamping to {}",
                self.video.framerate, MAX_FRAMERATE
            );
            self.video.framerate = MAX_FRAMERATE;
        }
        if self.video.bitrate_mbps == 0 {
            warn!("Config: bitrate_mbps was 0, clamping to 20");
            self.video.bitrate_mbps = 20;
        } else if self.video.bitrate_mbps > 500 {
            warn!(
                "Config: bitrate_mbps was {}, clamping to 500",
                self.video.bitrate_mbps
            );
            self.video.bitrate_mbps = 500;
        }
        if let Some(value) = self.video.quality_value {
            let clamped = value.clamp(1, 51);
            if clamped != value {
                warn!(
                    "Config: quality_value was {}, clamping to {}",
                    value, clamped
                );
                self.video.quality_value = Some(clamped);
            }
        }
        if matches!(self.video.rate_control, RateControl::Cq) && self.video.quality_value.is_none()
        {
            self.video.quality_value =
                Some(default_quality_value_for_preset(self.video.quality_preset));
        }
        if self.general.replay_duration_secs == 0 {
            warn!("Config: replay_duration_secs was 0, clamping to 30");
            self.general.replay_duration_secs = 30;
        }
        if self.advanced.keyframe_interval_secs == 0 {
            warn!("Config: keyframe_interval_secs was 0, clamping to 1");
            self.advanced.keyframe_interval_secs = 1;
        }
        if self.advanced.memory_limit_mb > MAX_REPLAY_MEMORY_LIMIT_MB {
            warn!(
                "Config: memory_limit_mb was {}, clamping to {}",
                self.advanced.memory_limit_mb, MAX_REPLAY_MEMORY_LIMIT_MB
            );
            self.advanced.memory_limit_mb = MAX_REPLAY_MEMORY_LIMIT_MB;
        }
        if !self.video.use_native_resolution && matches!(self.video.resolution, Resolution::Native)
        {
            warn!(
                "Config: use_native_resolution is false but resolution is 'native' - \
                 setting use_native_resolution to true"
            );
            self.video.use_native_resolution = true;
        }
        if self.video.use_native_resolution && !matches!(self.video.resolution, Resolution::Native)
        {
            warn!(
                "Config: use_native_resolution is true but resolution is set to {:?} - \
                 resolution setting will be ignored",
                self.video.resolution
            );
        }
        // Validate custom resolution dimensions
        if let Resolution::Custom(width, height) = self.video.resolution {
            const MIN_DIMENSION: u32 = 160;
            const MAX_DIMENSION: u32 = 8192;
            let mut adjusted_width = width;
            let mut adjusted_height = height;

            // Ensure dimensions are even (required by most hardware encoders)
            if width % 2 != 0 {
                warn!(
                    "Config: Custom resolution width {} is not divisible by 2, rounding to {}",
                    width,
                    width + 1
                );
                adjusted_width = width + 1;
            }
            if height % 2 != 0 {
                warn!(
                    "Config: Custom resolution height {} is not divisible by 2, rounding to {}",
                    height,
                    height + 1
                );
                adjusted_height = height + 1;
            }

            // Clamp to valid range
            adjusted_width = adjusted_width.clamp(MIN_DIMENSION, MAX_DIMENSION);
            adjusted_height = adjusted_height.clamp(MIN_DIMENSION, MAX_DIMENSION);

            if adjusted_width != width || adjusted_height != height {
                warn!(
                    "Config: Custom resolution adjusted from {}x{} to {}x{}",
                    width, height, adjusted_width, adjusted_height
                );
                self.video.resolution = Resolution::Custom(adjusted_width, adjusted_height);
            }
        }
        // Validate audio settings
        self.audio.balance = self.audio.balance.clamp(-100, 100);
        self.audio.master_volume = self.audio.master_volume.clamp(0, 200);
        self.audio.system_volume = self.audio.system_volume.clamp(0, 200);
        self.audio.mic_volume = self.audio.mic_volume.clamp(0, 400);
        self.audio.target_lufs = self.audio.target_lufs.clamp(-23, -14);
        self.audio.true_peak_limit_dbtp = self.audio.true_peak_limit_dbtp.clamp(-3, 0);
        // mic_noise_reduction is a simple on/off toggle, no per-parameter clamping required.

        // Validate save_directory for security and correctness
        self.validate_save_directory();

        crate::hotkey_parse::validate_hotkey_config_strings(self);
    }

    /// Validates the save_directory path for security issues.
    ///
    /// Checks for:
    /// - Path traversal attempts (e.g., ".." components)
    /// - Non-absolute paths
    /// - Ensures the path can be converted to a valid PathBuf
    fn validate_save_directory(&mut self) {
        use std::path::Path;
        use tracing::warn;

        let save_dir = &self.general.save_directory;
        let path = Path::new(save_dir);

        // Check for empty path
        if save_dir.is_empty() {
            warn!("Config: save_directory is empty, using default");
            self.general.save_directory = super::functions::default_save_directory();
            return;
        }

        // Check for path traversal attempts
        for component in path.components() {
            if matches!(component, std::path::Component::ParentDir) {
                warn!(
                    "Config: save_directory contains '..' (path traversal), using default: {}",
                    save_dir
                );
                self.general.save_directory = super::functions::default_save_directory();
                return;
            }
        }

        // Check for absolute path
        if !path.is_absolute() {
            warn!(
                "Config: save_directory is not absolute, using default: {}",
                save_dir
            );
            self.general.save_directory = super::functions::default_save_directory();
        }
    }

    pub fn estimated_replay_storage_bytes(&self) -> usize {
        let duration_secs = self.general.replay_duration_secs.max(1) as u64;
        let video_bps = (self.video.bitrate_mbps.max(1) as u64).saturating_mul(1_000_000);
        let system_audio_bps = if self.audio.capture_system {
            ESTIMATED_SYSTEM_AUDIO_BITRATE_BPS
        } else {
            0
        };
        let mic_audio_bps = if self.audio.capture_mic {
            ESTIMATED_MIC_AUDIO_BITRATE_BPS
        } else {
            0
        };
        let total_bps = video_bps
            .saturating_add(system_audio_bps)
            .saturating_add(mic_audio_bps);
        let total_bytes = total_bps
            .saturating_mul(duration_secs)
            .checked_div(8)
            .unwrap_or(u64::MAX);
        total_bytes.min(usize::MAX as u64) as usize
    }

    pub fn estimated_replay_storage_mb(&self) -> u32 {
        let bytes = self.estimated_replay_storage_bytes() as u64;
        bytes
            .saturating_add((1024 * 1024) - 1)
            .checked_div(1024 * 1024)
            .unwrap_or(u64::MAX)
            .min(u32::MAX as u64) as u32
    }

    pub fn recommended_replay_memory_limit_mb(&self) -> u32 {
        let estimated_bytes = self.estimated_replay_storage_bytes() as u64;
        let with_headroom = estimated_bytes
            .saturating_mul(RECOMMENDED_BUFFER_HEADROOM_PERCENT)
            .checked_div(100)
            .unwrap_or(u64::MAX)
            .saturating_add(RECOMMENDED_BUFFER_BASE_OVERHEAD_MB * 1024 * 1024);
        let recommended_mb = with_headroom
            .saturating_add((1024 * 1024) - 1)
            .checked_div(1024 * 1024)
            .unwrap_or(u64::MAX)
            .min(u32::MAX as u64) as u32;
        recommended_mb.clamp(MIN_REPLAY_MEMORY_LIMIT_MB, MAX_REPLAY_MEMORY_LIMIT_MB)
    }

    pub fn effective_replay_memory_limit_mb(&self) -> u32 {
        let requested = self.advanced.memory_limit_mb;
        if requested == REPLAY_MEMORY_LIMIT_AUTO_MB {
            self.recommended_replay_memory_limit_mb()
        } else {
            requested.clamp(MIN_REPLAY_MEMORY_LIMIT_MB, MAX_REPLAY_MEMORY_LIMIT_MB)
        }
    }

    pub fn requires_pipeline_restart(&self, other: &Config) -> bool {
        self.video.encoder != other.video.encoder
            || self.video.resolution != other.video.resolution
            || self.video.use_native_resolution != other.video.use_native_resolution
            || self.video.framerate != other.video.framerate
            || self.video.bitrate_mbps != other.video.bitrate_mbps
            || self.video.quality_preset != other.video.quality_preset
            || self.video.rate_control != other.video.rate_control
            || self.video.quality_value != other.video.quality_value
            || self.audio.capture_system != other.audio.capture_system
            || self.audio.capture_mic != other.audio.capture_mic
            || self.audio.mic_device != other.audio.mic_device
            || self.audio.mic_noise_reduction != other.audio.mic_noise_reduction
            || self.advanced.gpu_index != other.advanced.gpu_index
            || self.advanced.keyframe_interval_secs != other.advanced.keyframe_interval_secs
            || self.advanced.use_cpu_readback != other.advanced.use_cpu_readback
            || self.general.replay_duration_secs != other.general.replay_duration_secs
            || self.advanced.memory_limit_mb != other.advanced.memory_limit_mb
    }

    pub fn requires_hotkey_reregister(&self, other: &Config) -> bool {
        self.hotkeys.save_clip != other.hotkeys.save_clip
            || self.hotkeys.toggle_recording != other.hotkeys.toggle_recording
            || self.hotkeys.screenshot != other.hotkeys.screenshot
            || self.hotkeys.open_gallery != other.hotkeys.open_gallery
    }
}

/// Rate control mode preference
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RateControl {
    Cbr,
    Vbr,
    Cq,
}

/// Audio capture settings
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AudioConfig {
    #[serde(default = "default_true")]
    pub capture_system: bool,
    #[serde(default = "default_false")]
    pub capture_mic: bool,
    #[serde(default = "default_mic_device")]
    pub mic_device: String,
    #[serde(default = "default_mic_volume")]
    pub mic_volume: u16,
    #[serde(default = "default_system_volume")]
    pub system_volume: u8,
    #[serde(default = "default_balance")]
    pub balance: i8, // -100 (left) to 100 (right)
    #[serde(default = "default_master_volume")]
    pub master_volume: u8,
    #[serde(default = "default_true")]
    pub mic_noise_reduction: bool,
    #[serde(default = "default_audio_normalization_enabled")]
    pub normalization_enabled: bool,
    #[serde(default = "default_audio_target_lufs")]
    pub target_lufs: i8,
    #[serde(default = "default_true_peak_limiter_enabled")]
    pub true_peak_limiter_enabled: bool,
    #[serde(default = "default_true_peak_limit_dbtp")]
    pub true_peak_limit_dbtp: i8,
}
/// Global hotkey bindings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotkeyConfig {
    #[serde(default = "default_hotkey_save")]
    pub save_clip: String,
    #[serde(default = "default_hotkey_toggle")]
    pub toggle_recording: String,
    #[serde(default = "default_hotkey_screenshot")]
    pub screenshot: String,
    #[serde(default = "default_hotkey_gallery")]
    pub open_gallery: String,
}

impl From<&Config> for HotkeyConfig {
    fn from(config: &Config) -> Self {
        config.hotkeys.clone()
    }
}

/// Video capture and encoding settings (HEVC-only)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoConfig {
    #[serde(default = "default_resolution")]
    pub resolution: Resolution,
    #[serde(default = "default_framerate")]
    pub framerate: u32,
    #[serde(default = "default_bitrate")]
    pub bitrate_mbps: u32,
    #[serde(default = "default_encoder")]
    pub encoder: EncoderType,
    #[serde(default = "default_quality_preset")]
    pub quality_preset: QualityPreset,
    #[serde(default = "default_rate_control")]
    pub rate_control: RateControl,
    #[serde(default = "default_quality_value")]
    pub quality_value: Option<u8>,
    #[serde(default = "default_true")]
    pub use_native_resolution: bool,
}

impl VideoConfig {
    /// Returns the target resolution dimensions based on the configured resolution setting.
    ///
    /// Returns `None` if using native resolution (capture at desktop resolution).
    /// Otherwise returns the specific dimensions for the selected resolution preset.
    ///
    /// # Returns
    /// - `None` - Use native/desktop resolution
    /// - `Some((width, height))` - Use specified resolution dimensions
    ///
    /// # Resolution Presets
    /// - Standard 16:9: 480p, 720p, 1080p, 1440p, 2160p
    /// - Ultrawide 21:9: UW1080 (2560x1080), UW1440 (3440x1440), UW2160 (5120x2160)
    /// - Super Ultrawide 32:9: SuperUW (3840x1080), SuperUW1440 (5120x1440)
    /// - Custom: User-defined dimensions
    pub fn target_resolution(&self) -> Option<(u32, u32)> {
        if self.use_native_resolution {
            return None;
        }
        match self.resolution {
            Resolution::Native => None,
            Resolution::P480 => Some((854, 480)),
            Resolution::P720 => Some((1280, 720)),
            Resolution::P1080 => Some((1920, 1080)),
            Resolution::P1440 => Some((2560, 1440)),
            Resolution::P2160 => Some((3840, 2160)),
            Resolution::UW1080 => Some((2560, 1080)),
            Resolution::UW1440 => Some((3440, 1440)),
            Resolution::UW2160 => Some((5120, 2160)),
            Resolution::SuperUW => Some((3840, 1080)),
            Resolution::SuperUW1440 => Some((5120, 1440)),
            Resolution::Custom(width, height) => Some((width, height)),
        }
    }
}
/// Advanced settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvancedConfig {
    #[serde(default)]
    pub memory_limit_mb: u32,
    #[serde(default = "default_gpu_index")]
    pub gpu_index: u32,
    #[serde(default = "default_keyframe_interval")]
    pub keyframe_interval_secs: u32,
    #[serde(default = "default_false")]
    pub use_cpu_readback: bool,
}
/// General application settings
/// General application settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    #[serde(default = "default_replay_duration")]
    pub replay_duration_secs: u32,
    #[serde(default = "default_save_directory")]
    pub save_directory: String,
    #[serde(default = "default_true")]
    pub auto_start_with_windows: bool,
    #[serde(default = "default_true")]
    pub start_minimised: bool,
    #[serde(default = "default_true")]
    pub notifications: bool,
    #[serde(default = "default_true")]
    pub auto_detect_game: bool,
    /// When false, skip post-save gallery thumbnail (linked libav decode); use for A/B memory diagnosis or `LITECLIP_SKIP_THUMBNAIL=1`.
    #[serde(default = "default_true")]
    pub generate_clip_thumbnail: bool,
    /// When true, use software encoder (libx265) for clip export instead of hardware encoders.
    /// Defaults to false (use hardware acceleration when available).
    #[serde(default = "default_false")]
    pub use_software_encoder: bool,
    /// When set, directory with Parakeet ONNX model files for optional burned-in export subtitles.
    #[serde(default)]
    pub parakeet_model_directory: Option<String>,
    /// Default state for the gallery export "burn auto subtitles" checkbox.
    #[serde(default = "default_false", alias = "burn_subtitles_on_export")]
    pub burn_auto_subtitles_default: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> Config {
        Config::default()
    }

    #[test]
    fn test_requires_pipeline_restart_video_encoder() {
        let mut config1 = default_config();
        let mut config2 = default_config();

        config1.video.encoder = EncoderType::Nvenc;
        config2.video.encoder = EncoderType::Amf;

        assert!(config1.requires_pipeline_restart(&config2));
    }

    #[test]
    fn test_requires_pipeline_restart_video_framerate() {
        let mut config1 = default_config();
        let mut config2 = default_config();

        config1.video.framerate = 30;
        config2.video.framerate = 60;

        assert!(config1.requires_pipeline_restart(&config2));
    }

    #[test]
    fn test_requires_pipeline_restart_video_bitrate() {
        let mut config1 = default_config();
        let mut config2 = default_config();

        config1.video.bitrate_mbps = 20;
        config2.video.bitrate_mbps = 50;

        assert!(config1.requires_pipeline_restart(&config2));
    }

    #[test]
    fn test_requires_pipeline_restart_audio_capture_system() {
        let mut config1 = default_config();
        let mut config2 = default_config();

        config1.audio.capture_system = true;
        config2.audio.capture_system = false;

        assert!(config1.requires_pipeline_restart(&config2));
    }

    #[test]
    fn test_requires_pipeline_restart_audio_capture_mic() {
        let mut config1 = default_config();
        let mut config2 = default_config();

        config1.audio.capture_mic = true;
        config2.audio.capture_mic = false;

        assert!(config1.requires_pipeline_restart(&config2));
    }

    #[test]
    fn test_requires_pipeline_restart_replay_duration() {
        let mut config1 = default_config();
        let mut config2 = default_config();

        config1.general.replay_duration_secs = 30;
        config2.general.replay_duration_secs = 60;

        assert!(config1.requires_pipeline_restart(&config2));
    }

    #[test]
    fn test_requires_pipeline_restart_no_change() {
        let config1 = default_config();
        let config2 = default_config();

        assert!(!config1.requires_pipeline_restart(&config2));
    }

    #[test]
    fn test_requires_pipeline_restart_general_settings_dont_trigger() {
        let mut config1 = default_config();
        let mut config2 = default_config();

        config1.general.notifications = true;
        config2.general.notifications = false;
        config1.general.auto_start_with_windows = true;
        config2.general.auto_start_with_windows = false;

        assert!(!config1.requires_pipeline_restart(&config2));
    }

    #[test]
    fn test_requires_hotkey_reregister_changed() {
        let mut config1 = default_config();
        let mut config2 = default_config();

        config1.hotkeys.save_clip = "Alt+F9".to_string();
        config2.hotkeys.save_clip = "Alt+F10".to_string();

        assert!(config1.requires_hotkey_reregister(&config2));
    }

    #[test]
    fn test_requires_hotkey_reregister_no_change() {
        let config1 = default_config();
        let config2 = default_config();

        assert!(!config1.requires_hotkey_reregister(&config2));
    }

    #[test]
    fn test_requires_hotkey_reregister_all_fields() {
        let mut config1 = default_config();
        let mut config2 = default_config();

        config1.hotkeys.save_clip = "Alt+F1".to_string();
        config2.hotkeys.save_clip = "Alt+F2".to_string();
        assert!(config1.requires_hotkey_reregister(&config2));

        config1 = default_config();
        config2 = default_config();
        config1.hotkeys.toggle_recording = "Alt+F3".to_string();
        config2.hotkeys.toggle_recording = "Alt+F4".to_string();
        assert!(config1.requires_hotkey_reregister(&config2));

        config1 = default_config();
        config2 = default_config();
        config1.hotkeys.screenshot = "Alt+F5".to_string();
        config2.hotkeys.screenshot = "Alt+F6".to_string();
        assert!(config1.requires_hotkey_reregister(&config2));

        config1 = default_config();
        config2 = default_config();
        config1.hotkeys.open_gallery = "Alt+F7".to_string();
        config2.hotkeys.open_gallery = "Alt+F8".to_string();
        assert!(config1.requires_hotkey_reregister(&config2));
    }
}
