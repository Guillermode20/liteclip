use log::{error, info};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Quality preset controlling CRF and encoder preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Quality {
    /// Faster encoding, lower quality, smaller file size.
    Low,
    /// Balanced settings for most users.
    Medium,
    /// Good visual quality, larger file size.
    High,
    /// Highest visual quality, very large file size.
    Ultra,
}

#[allow(dead_code)]
impl Quality {
    pub fn crf(&self) -> u32 {
        match self {
            Quality::Low => 32,
            Quality::Medium => 26,
            Quality::High => 21,
            Quality::Ultra => 16,
        }
    }

    /// x264 preset string.
    pub fn preset(&self) -> &'static str {
        match self {
            Quality::Low => "ultrafast",
            Quality::Medium => "superfast",
            Quality::High => "veryfast",
            Quality::Ultra => "faster",
        }
    }

    /// NVENC preset (p1 = fastest, p7 = slowest/best quality).
    pub fn nvenc_preset(&self) -> &'static str {
        match self {
            Quality::Low => "p1",
            Quality::Medium => "p4",
            Quality::High => "p5",
            Quality::Ultra => "p6",
        }
    }

    /// Intel QSV preset.
    pub fn qsv_preset(&self) -> &'static str {
        match self {
            Quality::Low => "veryfast",
            Quality::Medium => "fast",
            Quality::High => "medium",
            Quality::Ultra => "slow",
        }
    }

    /// AMD AMF quality mode.
    pub fn amf_quality(&self) -> &'static str {
        match self {
            Quality::Low => "speed",
            Quality::Medium => "balanced",
            Quality::High => "quality",
            Quality::Ultra => "quality",
        }
    }

    /// Target bitrate used for hardware encoders (kbps).
    pub fn target_bitrate_kbps(&self) -> u32 {
        match self {
            Quality::Low => 4000,
            Quality::Medium => 7000,
            Quality::High => 11000,
            Quality::Ultra => 16000,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Quality::Low => "Low (small files)",
            Quality::Medium => "Medium",
            Quality::High => "High",
            Quality::Ultra => "Ultra (large files)",
        }
    }

    pub fn all() -> &'static [Quality] {
        &[Quality::Low, Quality::Medium, Quality::High, Quality::Ultra]
    }
}

/// Video encoder preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VideoEncoder {
    /// Automatically choose the best available hardware encoder.
    Auto,
    /// Software-based H.264 encoding (CPU bound).
    Libx264,
    /// NVIDIA hardware-accelerated H.264 encoding.
    H264Nvenc,
    /// Intel hardware-accelerated H.264 encoding (Quick Sync).
    H264Qsv,
    /// AMD hardware-accelerated H.264 encoding (Advanced Media Framework).
    H264Amf,
}

#[allow(dead_code)]
impl VideoEncoder {
    pub fn encoder_name(&self) -> Option<&'static str> {
        match self {
            VideoEncoder::Auto => None,
            VideoEncoder::Libx264 => Some("libx264"),
            VideoEncoder::H264Nvenc => Some("h264_nvenc"),
            VideoEncoder::H264Qsv => Some("h264_qsv"),
            VideoEncoder::H264Amf => Some("h264_amf"),
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            VideoEncoder::Auto => "Auto (recommended)",
            VideoEncoder::Libx264 => "Software (x264)",
            VideoEncoder::H264Nvenc => "NVIDIA NVENC",
            VideoEncoder::H264Qsv => "Intel Quick Sync",
            VideoEncoder::H264Amf => "AMD AMF",
        }
    }

    /// Whether this is a hardware-accelerated encoder.
    #[allow(dead_code)]
    pub fn is_hardware(&self) -> bool {
        matches!(
            self,
            VideoEncoder::H264Nvenc | VideoEncoder::H264Qsv | VideoEncoder::H264Amf
        )
    }

    pub fn all() -> &'static [VideoEncoder] {
        &[
            VideoEncoder::Auto,
            VideoEncoder::Libx264,
            VideoEncoder::H264Nvenc,
            VideoEncoder::H264Qsv,
            VideoEncoder::H264Amf,
        ]
    }

    pub fn resolve(&self, available: &[VideoEncoder]) -> VideoEncoder {
        match self {
            VideoEncoder::Auto => {
                if available.contains(&VideoEncoder::H264Nvenc) {
                    VideoEncoder::H264Nvenc
                } else if available.contains(&VideoEncoder::H264Qsv) {
                    VideoEncoder::H264Qsv
                } else if available.contains(&VideoEncoder::H264Amf) {
                    VideoEncoder::H264Amf
                } else {
                    VideoEncoder::Libx264
                }
            }
            explicit => {
                if *explicit == VideoEncoder::Libx264 || available.contains(explicit) {
                    *explicit
                } else {
                    VideoEncoder::Libx264
                }
            }
        }
    }
}

/// Rate control mode for video encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RateControl {
    /// Use the built-in quality presets (simple mode).
    Preset,
    /// Constant bitrate.
    Cbr,
    /// Variable bitrate with a target and max bitrate.
    Vbr,
    /// Constant quality (CRF-like). Best supported with software x264.
    Crf,
}

impl RateControl {
    pub fn label(&self) -> &'static str {
        match self {
            RateControl::Preset => "Preset (simple)",
            RateControl::Cbr => "CBR",
            RateControl::Vbr => "VBR",
            RateControl::Crf => "CRF (quality-based)",
        }
    }

    pub fn all() -> &'static [RateControl] {
        &[
            RateControl::Preset,
            RateControl::Cbr,
            RateControl::Vbr,
            RateControl::Crf,
        ]
    }
}

/// Generic encoder speed/quality tuning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EncoderTuning {
    Fastest,
    Fast,
    Balanced,
    Quality,
    MaxQuality,
}

#[allow(dead_code)]
impl EncoderTuning {
    pub fn label(&self) -> &'static str {
        match self {
            EncoderTuning::Fastest => "Fastest",
            EncoderTuning::Fast => "Fast",
            EncoderTuning::Balanced => "Balanced",
            EncoderTuning::Quality => "Quality",
            EncoderTuning::MaxQuality => "Max Quality",
        }
    }

    pub fn all() -> &'static [EncoderTuning] {
        &[
            EncoderTuning::Fastest,
            EncoderTuning::Fast,
            EncoderTuning::Balanced,
            EncoderTuning::Quality,
            EncoderTuning::MaxQuality,
        ]
    }

    pub fn x264_preset(&self) -> &'static str {
        match self {
            EncoderTuning::Fastest => "ultrafast",
            EncoderTuning::Fast => "veryfast",
            EncoderTuning::Balanced => "faster",
            EncoderTuning::Quality => "medium",
            EncoderTuning::MaxQuality => "slow",
        }
    }

    pub fn nvenc_preset(&self) -> &'static str {
        match self {
            EncoderTuning::Fastest => "p1",
            EncoderTuning::Fast => "p3",
            EncoderTuning::Balanced => "p4",
            EncoderTuning::Quality => "p5",
            EncoderTuning::MaxQuality => "p7",
        }
    }

    pub fn qsv_preset(&self) -> &'static str {
        match self {
            EncoderTuning::Fastest => "veryfast",
            EncoderTuning::Fast => "faster",
            EncoderTuning::Balanced => "fast",
            EncoderTuning::Quality => "medium",
            EncoderTuning::MaxQuality => "slow",
        }
    }

    pub fn amf_quality(&self) -> &'static str {
        match self {
            EncoderTuning::Fastest => "speed",
            EncoderTuning::Fast => "speed",
            EncoderTuning::Balanced => "balanced",
            EncoderTuning::Quality => "quality",
            EncoderTuning::MaxQuality => "quality",
        }
    }
}

/// Framerate preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Framerate {
    /// 15 frames per second.
    Fps15,
    /// 30 frames per second.
    Fps30,
    /// 60 frames per second.
    Fps60,
}

impl Framerate {
    pub fn value(&self) -> u32 {
        match self {
            Framerate::Fps15 => 15,
            Framerate::Fps30 => 30,
            Framerate::Fps60 => 60,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Framerate::Fps15 => "15 FPS",
            Framerate::Fps30 => "30 FPS",
            Framerate::Fps60 => "60 FPS",
        }
    }

    pub fn all() -> &'static [Framerate] {
        &[Framerate::Fps15, Framerate::Fps30, Framerate::Fps60]
    }
}

/// Resolution preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Resolution {
    /// Use the native resolution of the display.
    Native,
    /// 1920x1080 resolution.
    Res1080p,
    /// 1280x720 resolution.
    Res720p,
    /// 854x480 resolution.
    Res480p,
}

#[allow(dead_code)]
impl Resolution {
    /// Returns the scale filter string, or None for native.
    /// Uses `-2` for height to ensure divisible-by-2 output — required by
    /// many hardware encoders that reject odd-dimension frames.
    pub fn scale_filter(&self) -> Option<&'static str> {
        match self {
            Resolution::Native => None,
            Resolution::Res1080p => Some("scale=1920:-2"),
            Resolution::Res720p => Some("scale=1280:-2"),
            Resolution::Res480p => Some("scale=854:-2"),
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Resolution::Native => "Native",
            Resolution::Res1080p => "1080p",
            Resolution::Res720p => "720p",
            Resolution::Res480p => "480p",
        }
    }

    pub fn all() -> &'static [Resolution] {
        &[
            Resolution::Native,
            Resolution::Res1080p,
            Resolution::Res720p,
            Resolution::Res480p,
        ]
    }
}

/// Hotkey preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HotkeyPreset {
    /// F8 key.
    F8,
    /// F9 key.
    F9,
    /// F10 key.
    F10,
    /// Ctrl+Shift+S key combination.
    CtrlShiftS,
    /// Alt+F9 key combination.
    AltF9,
}

impl HotkeyPreset {
    pub fn label(&self) -> &'static str {
        match self {
            HotkeyPreset::F8 => "F8",
            HotkeyPreset::F9 => "F9",
            HotkeyPreset::F10 => "F10",
            HotkeyPreset::CtrlShiftS => "Ctrl+Shift+S",
            HotkeyPreset::AltF9 => "Alt+F9",
        }
    }

    pub fn all() -> &'static [HotkeyPreset] {
        &[
            HotkeyPreset::F8,
            HotkeyPreset::F9,
            HotkeyPreset::F10,
            HotkeyPreset::CtrlShiftS,
            HotkeyPreset::AltF9,
        ]
    }
}

/// All app settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Encoding quality level.
    pub quality: Quality,
    /// Preferred video encoder.
    pub video_encoder: VideoEncoder,
    /// Target framerate for the recording.
    pub framerate: Framerate,
    /// Target resolution (Native or downscaled).
    pub resolution: Resolution,
    /// Enables advanced encoding controls for power users.
    #[serde(default = "default_advanced_video_controls")]
    pub advanced_video_controls: bool,
    /// Rate control mode (preset/CBR/VBR/CRF).
    #[serde(default = "default_rate_control")]
    pub rate_control: RateControl,
    /// Encoder speed/quality tuning.
    #[serde(default = "default_encoder_tuning")]
    pub encoder_tuning: EncoderTuning,
    /// Target video bitrate in kbps.
    #[serde(default = "default_video_bitrate_kbps")]
    pub video_bitrate_kbps: u32,
    /// Max video bitrate in kbps (used for VBR and hardware rate control).
    #[serde(default = "default_video_max_bitrate_kbps")]
    pub video_max_bitrate_kbps: u32,
    /// Rate control buffer size in kbps.
    #[serde(default = "default_video_bufsize_kbps")]
    pub video_bufsize_kbps: u32,
    /// CRF value for software x264 when using CRF mode.
    #[serde(default = "default_video_crf")]
    pub video_crf: u32,
    /// GOP/keyframe interval in seconds.
    #[serde(default = "default_keyframe_interval_sec")]
    pub keyframe_interval_sec: u32,
    /// Custom output resolution toggle.
    #[serde(default = "default_custom_resolution_enabled")]
    pub custom_resolution_enabled: bool,
    /// Custom output width in pixels.
    #[serde(default = "default_custom_resolution_width")]
    pub custom_resolution_width: u32,
    /// Custom output height in pixels.
    #[serde(default = "default_custom_resolution_height")]
    pub custom_resolution_height: u32,
    /// Audio bitrate in kbps.
    #[serde(default = "default_audio_bitrate_kbps")]
    pub audio_bitrate_kbps: u32,
    /// Rolling buffer length in seconds.
    pub buffer_seconds: u64,
    /// Whether to capture desktop audio.
    pub capture_audio: bool,
    /// Selected audio device name (None = auto-detect first).
    pub audio_device: Option<String>,
    /// Directory to save clips to.
    pub output_dir: PathBuf,
    /// Global hotkey preset for saving clips.
    pub hotkey: HotkeyPreset,
    /// Whether to launch the app on Windows startup.
    #[serde(default)]
    pub launch_on_startup: bool,
    /// Whether to minimize to system tray instead of closing.
    #[serde(default)]
    pub minimize_to_tray: bool,
}

impl Default for Settings {
    fn default() -> Self {
        let output_dir = dirs::video_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
            .join("LiteClipReplay");

        Self {
            quality: Quality::Medium,
            video_encoder: VideoEncoder::Auto,
            framerate: Framerate::Fps30,
            resolution: Resolution::Native,
            advanced_video_controls: default_advanced_video_controls(),
            rate_control: default_rate_control(),
            encoder_tuning: default_encoder_tuning(),
            video_bitrate_kbps: default_video_bitrate_kbps(),
            video_max_bitrate_kbps: default_video_max_bitrate_kbps(),
            video_bufsize_kbps: default_video_bufsize_kbps(),
            video_crf: default_video_crf(),
            keyframe_interval_sec: default_keyframe_interval_sec(),
            custom_resolution_enabled: default_custom_resolution_enabled(),
            custom_resolution_width: default_custom_resolution_width(),
            custom_resolution_height: default_custom_resolution_height(),
            audio_bitrate_kbps: default_audio_bitrate_kbps(),
            buffer_seconds: 120,
            capture_audio: true,
            audio_device: None,
            output_dir,
            hotkey: HotkeyPreset::F8,
            launch_on_startup: false,
            minimize_to_tray: false,
        }
    }
}

#[allow(dead_code)]
impl Settings {
    /// Returns the active scale filter string, if any.
    pub fn active_scale_filter(&self) -> Option<String> {
        if self.custom_resolution_enabled {
            let width = clamp_even(self.custom_resolution_width, 320, 7680);
            let height = clamp_even(self.custom_resolution_height, 240, 4320);
            Some(format!("scale={}:{}", width, height))
        } else {
            self.resolution.scale_filter().map(str::to_string)
        }
    }

    /// Load settings from disk, or return default if missing/invalid.
    pub fn load() -> Self {
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("LiteClipReplay");
        let settings_path = config_dir.join("settings.json");

        if let Ok(file) = std::fs::File::open(&settings_path) {
            let reader = std::io::BufReader::new(file);
            if let Ok(settings) = serde_json::from_reader(reader) {
                return settings;
            }
        }

        Self::default()
    }

    /// Save current settings to disk.
    pub fn save(&self) {
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("LiteClipReplay");
        let _ = std::fs::create_dir_all(&config_dir);
        let settings_path = config_dir.join("settings.json");

        if let Ok(file) = std::fs::File::create(&settings_path) {
            let writer = std::io::BufWriter::new(file);
            let _ = serde_json::to_writer_pretty(writer, self);
        }
    }
}

#[allow(dead_code)]
fn clamp_even(value: u32, min: u32, max: u32) -> u32 {
    let clamped = value.clamp(min, max);
    if clamped % 2 == 0 {
        clamped
    } else if clamped == max {
        clamped.saturating_sub(1)
    } else {
        clamped + 1
    }
}

fn default_advanced_video_controls() -> bool {
    false
}

fn default_rate_control() -> RateControl {
    RateControl::Preset
}

fn default_encoder_tuning() -> EncoderTuning {
    EncoderTuning::Balanced
}

fn default_video_bitrate_kbps() -> u32 {
    8000
}

fn default_video_max_bitrate_kbps() -> u32 {
    12000
}

fn default_video_bufsize_kbps() -> u32 {
    16000
}

fn default_video_crf() -> u32 {
    23
}

fn default_keyframe_interval_sec() -> u32 {
    2
}

fn default_custom_resolution_enabled() -> bool {
    false
}

fn default_custom_resolution_width() -> u32 {
    1920
}

fn default_custom_resolution_height() -> u32 {
    1080
}

fn default_audio_bitrate_kbps() -> u32 {
    128
}

/// Add or remove the app from Windows startup via the registry.
///
/// Writes to `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
pub fn set_launch_on_startup(enabled: bool) {
    use winreg::enums::*;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let run_key = match hkcu.open_subkey_with_flags(
        r"Software\Microsoft\Windows\CurrentVersion\Run",
        KEY_SET_VALUE | KEY_READ,
    ) {
        Ok(key) => key,
        Err(e) => {
            error!("Failed to open Run registry key: {}", e);
            return;
        }
    };

    if enabled {
        match std::env::current_exe() {
            Ok(exe_path) => {
                let exe_str = format!("\"{}\"", exe_path.display());
                match run_key.set_value("LiteClip", &exe_str) {
                    Ok(()) => info!("Added LiteClip to Windows startup: {}", exe_str),
                    Err(e) => error!("Failed to set startup registry value: {}", e),
                }
            }
            Err(e) => error!("Failed to get current exe path: {}", e),
        }
    } else {
        match run_key.delete_value("LiteClip") {
            Ok(()) => info!("Removed LiteClip from Windows startup"),
            Err(e) => {
                // Not an error if it wasn't there
                info!("Could not remove startup entry (may not exist): {}", e);
            }
        }
    }
}
