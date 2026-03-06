//! Auto-generated module
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::functions::{
    default_bitrate, default_codec, default_encoder, default_false, default_framerate,
    default_gpu_index, default_hotkey_gallery, default_hotkey_save, default_hotkey_screenshot,
    default_hotkey_toggle, default_keyframe_interval, default_memory_limit, default_mic_device,
    default_mic_volume, default_overlay_position, default_quality_preset, default_quality_value,
    default_quality_value_for_preset, default_rate_control, default_replay_duration,
    default_resolution, default_save_directory, default_system_volume, default_true,
    ESTIMATED_MIC_AUDIO_BITRATE_BPS, ESTIMATED_SYSTEM_AUDIO_BITRATE_BPS,
    LEGACY_DEFAULT_MEMORY_LIMIT_MB, MAX_FRAMERATE, MAX_MEMORY_LIMIT_MB, MIN_MEMORY_LIMIT_MB,
    RECOMMENDED_BUFFER_BASE_OVERHEAD_MB, RECOMMENDED_BUFFER_HEADROOM_PERCENT,
};

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
impl Config {
    /// Load configuration from file or create default
    pub async fn load() -> Result<Self> {
        let config_path = Self::config_path()?;
        if config_path.exists() {
            let content = tokio::fs::read_to_string(&config_path)
                .await
                .with_context(|| format!("Failed to read config from {:?}", config_path))?;
            let mut config: Config = toml::from_str(&content)?;
            config.validate();
            Ok(config)
        } else {
            let mut config = Config::default();
            config.validate();
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

    /// Save synchronously — used from the GUI thread which has no tokio runtime.
    pub fn save_sync(&self) -> Result<()> {
        let config_path = Self::config_path()?;
        let parent = config_path
            .parent()
            .context("Config path has no parent directory")?;
        std::fs::create_dir_all(parent).context("Failed to create config directory")?;
        let content = toml::to_string_pretty(self)?;
        std::fs::write(&config_path, &content)
            .with_context(|| format!("Failed to write config to {:?}", config_path))?;
        Ok(())
    }

    /// Load synchronously — used from the GUI thread which has no tokio runtime.
    pub fn load_sync() -> Result<Self> {
        let config_path = Self::config_path()?;
        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)
                .with_context(|| format!("Failed to read config from {:?}", config_path))?;
            let mut config: Self = toml::from_str(&content)
                .with_context(|| format!("Failed to parse config from {:?}", config_path))?;
            config.validate();
            Ok(config)
        } else {
            Ok(Self::default())
        }
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
        if self.advanced.memory_limit_mb == LEGACY_DEFAULT_MEMORY_LIMIT_MB {
            let recommended = self.recommended_replay_memory_limit_mb();
            warn!(
                "Config: migrating legacy memory_limit_mb={} to recommended {} MB",
                LEGACY_DEFAULT_MEMORY_LIMIT_MB, recommended
            );
            self.advanced.memory_limit_mb = recommended;
        } else if self.advanced.memory_limit_mb == 0 {
            let recommended = self.recommended_replay_memory_limit_mb();
            warn!(
                "Config: memory_limit_mb was 0, clamping to recommended {} MB",
                recommended
            );
            self.advanced.memory_limit_mb = recommended;
        } else if self.advanced.memory_limit_mb < MIN_MEMORY_LIMIT_MB {
            warn!(
                "Config: memory_limit_mb was {}, clamping to {}",
                self.advanced.memory_limit_mb, MIN_MEMORY_LIMIT_MB
            );
            self.advanced.memory_limit_mb = MIN_MEMORY_LIMIT_MB;
        } else if self.advanced.memory_limit_mb > MAX_MEMORY_LIMIT_MB {
            warn!(
                "Config: memory_limit_mb was {}, clamping to {}",
                self.advanced.memory_limit_mb, MAX_MEMORY_LIMIT_MB
            );
            self.advanced.memory_limit_mb = MAX_MEMORY_LIMIT_MB;
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
            .clamp(MIN_MEMORY_LIMIT_MB as u64, MAX_MEMORY_LIMIT_MB as u64);
        recommended_mb as u32
    }

    pub fn effective_replay_memory_limit_mb(&self) -> u32 {
        self.advanced
            .memory_limit_mb
            .clamp(MIN_MEMORY_LIMIT_MB, MAX_MEMORY_LIMIT_MB)
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
    #[serde(default = "default_false")]
    pub mic_noise_reduction: bool,
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
    #[serde(default = "default_true")]
    pub use_native_resolution: bool,
}

impl VideoConfig {
    pub fn target_resolution(&self) -> Option<(u32, u32)> {
        if self.use_native_resolution {
            return None;
        }
        match self.resolution {
            Resolution::Native => None,
            Resolution::P1080 => Some((1920, 1080)),
            Resolution::P720 => Some((1280, 720)),
            Resolution::P480 => Some((854, 480)),
        }
    }
}
/// Video codec options
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Codec {
    H264,
    H265,
    Av1,
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
/// Overlay position options
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OverlayPosition {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}
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
