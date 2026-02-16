//! TOML Configuration Management
//!
//! Configuration stored in %APPDATA%/liteclip-replay/liteclip-replay.toml

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Root configuration structure
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub general: GeneralConfig,
    #[serde(default)]
    pub video: VideoConfig,
    #[serde(default)]
    pub audio: AudioConfig,
    #[serde(default)]
    pub hotkeys: HotkeyConfig,
    #[serde(default)]
    pub advanced: AdvancedConfig,
}

const MAX_FRAMERATE: u32 = 240;

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
}

/// Video capture and encoding settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoConfig {
    #[serde(default = "default_resolution")]
    pub resolution: Resolution,
    #[serde(default = "default_framerate")]
    pub framerate: u32,
    #[serde(default = "default_codec")]
    pub codec: Codec,
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
}

/// Audio capture settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    #[serde(default = "default_true")]
    pub capture_system: bool,
    #[serde(default = "default_false")]
    pub capture_mic: bool,
    #[serde(default = "default_mic_device")]
    pub mic_device: String,
    #[serde(default = "default_mic_volume")]
    pub mic_volume: u8,
    #[serde(default = "default_system_volume")]
    pub system_volume: u8,
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

/// Advanced settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvancedConfig {
    #[serde(default = "default_memory_limit")]
    pub memory_limit_mb: u32,
    #[serde(default = "default_gpu_index")]
    pub gpu_index: u32,
    #[serde(default = "default_keyframe_interval")]
    pub keyframe_interval_secs: u32,
    #[serde(default = "default_true")]
    pub overlay_enabled: bool,
    #[serde(default = "default_overlay_position")]
    pub overlay_position: OverlayPosition,
    #[serde(default = "default_false")]
    pub use_cpu_readback: bool,
}

/// Resolution options
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    Native,
    #[serde(rename = "1080p")]
    P1080,
    #[serde(rename = "720p")]
    P720,
    #[serde(rename = "480p")]
    P480,
}

/// Video codec options
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Codec {
    H264,
    H265,
    Av1,
}

/// Encoder selection
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EncoderType {
    Auto,
    Nvenc,
    Amf,
    Qsv,
    Software,
}

/// High-level quality/speed tradeoff for encoder options
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QualityPreset {
    Performance,
    Balanced,
    Quality,
}

/// Rate control mode preference
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RateControl {
    Cbr,
    Vbr,
    Cq,
}

/// Overlay position options
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OverlayPosition {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Config {
    /// Load configuration from file or create default
    pub async fn load() -> Result<Self> {
        let config_path = Self::config_path()?;

        if config_path.exists() {
            let content = tokio::fs::read_to_string(&config_path)
                .await
                .with_context(|| format!("Failed to read config from {:?}", config_path))?;
            let config: Config = toml::from_str(&content)?;
            Ok(config)
        } else {
            let config = Config::default();
            config.save().await?;
            Ok(config)
        }
    }

    /// Save configuration to file
    pub async fn save(&self) -> Result<()> {
        let config_path = Self::config_path()?;
        let parent = config_path
            .parent()
            .context("Config path has no parent directory")?;

        tokio::fs::create_dir_all(parent).await?;

        let content = toml::to_string_pretty(self)?;
        tokio::fs::write(&config_path, content)
            .await
            .with_context(|| format!("Failed to write config to {:?}", config_path))?;

        Ok(())
    }

    /// Get the configuration file path
    pub fn config_path() -> Result<PathBuf> {
        let app_data = dirs::data_dir().context("Failed to get data directory")?;
        Ok(app_data
            .join("liteclip-replay")
            .join("liteclip-replay.toml"))
    }

    /// Validate and sanitize configuration values
    ///
    /// Clamps values to safe ranges to prevent panics from invalid user input.
    pub fn validate(&mut self) {
        use tracing::warn;

        // Framerate must be > 0 (prevents division by zero in capture and encoder)
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

        // Memory limit must be in a sane range
        if self.advanced.memory_limit_mb == 0 {
            warn!("Config: memory_limit_mb was 0, clamping to 512");
            self.advanced.memory_limit_mb = 512;
        } else if self.advanced.memory_limit_mb > 16384 {
            warn!(
                "Config: memory_limit_mb was {}, clamping to 16384",
                self.advanced.memory_limit_mb
            );
            self.advanced.memory_limit_mb = 16384;
        }

        // Bitrate must be > 0
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

        // Quality value (CQ/CRF-like scalar) must be in a safe range if provided
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

        // Provide a deterministic quality value when CQ mode is selected
        if matches!(self.video.rate_control, RateControl::Cq) && self.video.quality_value.is_none()
        {
            self.video.quality_value =
                Some(default_quality_value_for_preset(self.video.quality_preset));
        }

        // Replay duration must be > 0
        if self.general.replay_duration_secs == 0 {
            warn!("Config: replay_duration_secs was 0, clamping to 30");
            self.general.replay_duration_secs = 30;
        }

        // Keyframe interval must be > 0
        if self.advanced.keyframe_interval_secs == 0 {
            warn!("Config: keyframe_interval_secs was 0, clamping to 1");
            self.advanced.keyframe_interval_secs = 1;
        }
    }
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            replay_duration_secs: default_replay_duration(),
            save_directory: default_save_directory(),
            auto_start_with_windows: default_true(),
            start_minimised: default_true(),
            notifications: default_true(),
            auto_detect_game: default_true(),
        }
    }
}

impl Default for VideoConfig {
    fn default() -> Self {
        Self {
            resolution: default_resolution(),
            framerate: default_framerate(),
            codec: default_codec(),
            bitrate_mbps: default_bitrate(),
            encoder: default_encoder(),
            quality_preset: default_quality_preset(),
            rate_control: default_rate_control(),
            quality_value: default_quality_value(),
        }
    }
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            capture_system: default_true(),
            capture_mic: default_true(),
            mic_device: default_mic_device(),
            mic_volume: default_mic_volume(),
            system_volume: default_system_volume(),
        }
    }
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            save_clip: default_hotkey_save(),
            toggle_recording: default_hotkey_toggle(),
            screenshot: default_hotkey_screenshot(),
            open_gallery: default_hotkey_gallery(),
        }
    }
}

impl Default for AdvancedConfig {
    fn default() -> Self {
        Self {
            memory_limit_mb: default_memory_limit(),
            gpu_index: default_gpu_index(),
            keyframe_interval_secs: default_keyframe_interval(),
            overlay_enabled: default_true(),
            overlay_position: default_overlay_position(),
            use_cpu_readback: default_false(),
        }
    }
}

// Default value functions
fn default_replay_duration() -> u32 {
    120
}

fn default_save_directory() -> String {
    dirs::video_dir()
        .map(|p| p.join("liteclip-replay").to_string_lossy().to_string())
        .unwrap_or_else(|| {
            // Avoid tilde paths on Windows — they don't expand automatically
            if let Ok(profile) = std::env::var("USERPROFILE") {
                format!("{}\\Videos\\liteclip-replay", profile)
            } else {
                "C:\\Videos\\liteclip-replay".to_string()
            }
        })
}

fn default_resolution() -> Resolution {
    Resolution::Native
}

fn default_framerate() -> u32 {
    60 // was 30 - increased for smoother playback
}

fn default_codec() -> Codec {
    Codec::H264
}

fn default_bitrate() -> u32 {
    50 // was 20 - increased for better quality (50 Mbps vs 20 Mbps)
}

fn default_encoder() -> EncoderType {
    EncoderType::Auto
}

fn default_quality_preset() -> QualityPreset {
    QualityPreset::Balanced
}

fn default_rate_control() -> RateControl {
    RateControl::Vbr
}

fn default_quality_value() -> Option<u8> {
    None
}

fn default_quality_value_for_preset(preset: QualityPreset) -> u8 {
    match preset {
        QualityPreset::Performance => 28,
        QualityPreset::Balanced => 23,
        QualityPreset::Quality => 19,
    }
}

fn default_mic_device() -> String {
    "default".to_string()
}

fn default_mic_volume() -> u8 {
    80
}

fn default_system_volume() -> u8 {
    100
}

fn default_hotkey_save() -> String {
    "Alt+F9".to_string()
}

fn default_hotkey_toggle() -> String {
    "Alt+F10".to_string()
}

fn default_hotkey_screenshot() -> String {
    "Alt+F11".to_string()
}

fn default_hotkey_gallery() -> String {
    "Alt+G".to_string()
}

fn default_memory_limit() -> u32 {
    2048 // was 512 - increased for more frame storage (2GB vs 512MB)
}

fn default_gpu_index() -> u32 {
    0
}

fn default_keyframe_interval() -> u32 {
    1
}

fn default_overlay_position() -> OverlayPosition {
    OverlayPosition::TopLeft
}

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.general.replay_duration_secs, 120);
        assert_eq!(config.video.framerate, 60);
        assert_eq!(config.video.codec, Codec::H264);
        assert_eq!(config.video.quality_preset, QualityPreset::Balanced);
        assert_eq!(config.video.rate_control, RateControl::Vbr);
        assert_eq!(config.video.quality_value, None);
        assert!(config.audio.capture_system);
        assert!(config.audio.capture_mic);
    }

    #[test]
    fn test_validate_quality_value_clamps() {
        let mut config = Config::default();
        config.video.rate_control = RateControl::Cq;
        config.video.quality_value = Some(0);

        config.validate();
        assert_eq!(config.video.quality_value, Some(1));

        config.video.quality_value = Some(99);
        config.validate();
        assert_eq!(config.video.quality_value, Some(51));
    }

    #[test]
    fn test_validate_cq_sets_default_quality_value() {
        let mut config = Config::default();
        config.video.rate_control = RateControl::Cq;
        config.video.quality_preset = QualityPreset::Quality;
        config.video.quality_value = None;

        config.validate();
        assert_eq!(config.video.quality_value, Some(19));
    }

    #[test]
    fn test_validate_framerate_upper_clamp() {
        let mut config = Config::default();
        config.video.framerate = 1000;

        config.validate();
        assert_eq!(config.video.framerate, MAX_FRAMERATE);
    }
}
