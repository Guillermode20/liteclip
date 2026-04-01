//! Auto-generated module
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use super::types::{EncoderType, QualityPreset, RateControl, Resolution};

pub const MAX_FRAMERATE: u32 = 240;
pub const RECOMMENDED_BUFFER_HEADROOM_PERCENT: u64 = 135;
pub const RECOMMENDED_BUFFER_BASE_OVERHEAD_MB: u64 = 24;
pub const ESTIMATED_SYSTEM_AUDIO_BITRATE_BPS: u64 = 192_000;
pub const ESTIMATED_MIC_AUDIO_BITRATE_BPS: u64 = 128_000;
pub const REPLAY_MEMORY_LIMIT_AUTO_MB: u32 = 0;
pub const MIN_REPLAY_MEMORY_LIMIT_MB: u32 = 1;
pub const MAX_REPLAY_MEMORY_LIMIT_MB: u32 = 4096;

pub(crate) fn default_true() -> bool {
    true
}

pub(crate) fn default_false() -> bool {
    false
}

pub(super) fn default_replay_duration() -> u32 {
    30
}
pub(super) fn default_save_directory() -> String {
    dirs::video_dir()
        .map(|p| p.join("liteclip").to_string_lossy().to_string())
        .unwrap_or_else(|| {
            if let Ok(profile) = std::env::var("USERPROFILE") {
                format!("{}\\Videos\\liteclip", profile)
            } else {
                "C:\\Videos\\liteclip".to_string()
            }
        })
}
pub(super) fn default_resolution() -> Resolution {
    Resolution::P1080
}
pub(super) fn default_framerate() -> u32 {
    60
}
pub(super) fn default_bitrate() -> u32 {
    25
}
pub(super) fn default_encoder() -> EncoderType {
    EncoderType::Auto
}
pub(super) fn default_quality_preset() -> QualityPreset {
    QualityPreset::Performance
}
pub(super) fn default_rate_control() -> RateControl {
    RateControl::Vbr
}
pub(super) fn default_quality_value() -> Option<u8> {
    None
}
pub(crate) fn default_quality_value_for_preset(preset: QualityPreset) -> u8 {
    match preset {
        QualityPreset::Performance => 28,
        QualityPreset::Balanced => 23,
        QualityPreset::Quality => 19,
    }
}
pub(super) fn default_mic_device() -> String {
    "default".to_string()
}
pub(super) fn default_mic_volume() -> u16 {
    100
}
pub(super) fn default_system_volume() -> u8 {
    72
}
pub(super) fn default_audio_normalization_enabled() -> bool {
    true
}
pub(super) fn default_audio_target_lufs() -> i8 {
    -16
}
pub(crate) fn default_balance() -> i8 {
    0
}
pub(crate) fn default_master_volume() -> u8 {
    100
}
pub(super) fn default_true_peak_limiter_enabled() -> bool {
    true
}
pub(super) fn default_true_peak_limit_dbtp() -> i8 {
    -1
}
pub(super) fn default_hotkey_save() -> String {
    "Ctrl+Shift+S".to_string()
}
pub(super) fn default_hotkey_toggle() -> String {
    "Ctrl+Shift+R".to_string()
}
pub(super) fn default_hotkey_screenshot() -> String {
    "Ctrl+Shift+P".to_string()
}
pub(super) fn default_hotkey_gallery() -> String {
    "Ctrl+Shift+G".to_string()
}
pub(super) fn default_gpu_index() -> u32 {
    0
}
pub(super) fn default_keyframe_interval() -> u32 {
    2
}

#[cfg(test)]
mod tests {
    use super::super::types::Config;
    use super::*;
    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.general.replay_duration_secs, 30);
        assert_eq!(config.video.framerate, 60);
        assert_eq!(config.video.bitrate_mbps, 25);
        assert_eq!(config.video.quality_preset, QualityPreset::Performance);
        assert_eq!(config.video.rate_control, RateControl::Vbr);
        assert_eq!(config.video.quality_value, None);
        assert_eq!(config.advanced.memory_limit_mb, 0);
        assert!(config.audio.capture_system);
        assert!(config.audio.capture_mic);
        assert_eq!(config.audio.system_volume, 72);
        assert_eq!(config.audio.mic_volume, 100);
        assert!(config.audio.mic_noise_reduction);
        assert!(config.audio.normalization_enabled);
        assert_eq!(config.audio.target_lufs, -16);
        assert!(config.audio.true_peak_limiter_enabled);
        assert_eq!(config.audio.true_peak_limit_dbtp, -1);
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

    #[test]
    fn test_validate_mic_volume_upper_clamp() {
        let mut config = Config::default();
        config.audio.mic_volume = u16::MAX;
        config.validate();
        assert_eq!(config.audio.mic_volume, 400);
    }

    #[test]
    fn test_recommended_replay_memory_limit_tracks_payload() {
        let mut config = Config::default();
        config.general.replay_duration_secs = 120;
        config.video.bitrate_mbps = 10;
        config.audio.capture_system = true;
        config.audio.capture_mic = false;

        let estimated_mb = config.estimated_replay_storage_mb();
        let recommended_mb = config.recommended_replay_memory_limit_mb();

        assert!(estimated_mb >= 140);
        assert!(recommended_mb > estimated_mb);
    }

    #[test]
    fn test_effective_replay_memory_limit_uses_recommended_when_auto() {
        let mut config = Config::default();
        config.advanced.memory_limit_mb = REPLAY_MEMORY_LIMIT_AUTO_MB;

        let expected = config
            .recommended_replay_memory_limit_mb()
            .clamp(MIN_REPLAY_MEMORY_LIMIT_MB, MAX_REPLAY_MEMORY_LIMIT_MB);
        assert_eq!(config.effective_replay_memory_limit_mb(), expected);
    }

    #[test]
    fn test_effective_replay_memory_limit_clamps_manual_value() {
        let mut config = Config::default();
        config.advanced.memory_limit_mb = MAX_REPLAY_MEMORY_LIMIT_MB + 1024;

        assert_eq!(
            config.effective_replay_memory_limit_mb(),
            MAX_REPLAY_MEMORY_LIMIT_MB
        );
    }

    #[test]
    fn test_validate_memory_limit_clamps_upper_bound() {
        let mut config = Config::default();
        config.advanced.memory_limit_mb = MAX_REPLAY_MEMORY_LIMIT_MB + 1;
        config.validate();
        assert_eq!(config.advanced.memory_limit_mb, MAX_REPLAY_MEMORY_LIMIT_MB);
    }
}
