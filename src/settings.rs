use std::path::PathBuf;

/// Quality preset controlling CRF and encoder preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    pub fn preset(&self) -> &'static str {
        match self {
            Quality::Low => "ultrafast",
            Quality::Medium => "superfast",
            Quality::High => "veryfast",
            Quality::Ultra => "faster",
        }
    }

    /// Target bitrate used for hardware encoders.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    pub fn scale_filter(&self) -> Option<&'static str> {
        match self {
            Resolution::Native => None,
            Resolution::Res1080p => Some("scale=1920:1080"),
            Resolution::Res720p => Some("scale=1280:720"),
            Resolution::Res480p => Some("scale=854:480"),
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone)]
pub struct Settings {
    /// Encoding quality level.
    pub quality: Quality,
    /// Preferred video encoder.
    pub video_encoder: VideoEncoder,
    /// Target framerate for the recording.
    pub framerate: Framerate,
            .join("LiteClip");

        Self {
            quality: Quality::Medium,
            video_encoder: VideoEncoder::Auto,
            framerate: Framerate::Fps30,
            resolution: Resolution::Native,
            buffer_seconds: 120,
            capture_audio: true,
            audio_device: None, // auto-detect
            output_dir,
            hotkey: HotkeyPreset::F8,
        }
    }
}
