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

impl VideoEncoder {
    pub fn ffmpeg_name(&self) -> Option<&'static str> {
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

impl Resolution {
    /// Returns the FFmpeg scale filter string, or None for native.
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

impl Settings {
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
                let exe_str = exe_path.display().to_string();
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
