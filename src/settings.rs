use std::path::PathBuf;

/// Quality preset controlling CRF and encoder preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    Low,
    Medium,
    High,
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
            Quality::Medium => "veryfast",
            Quality::High => "fast",
            Quality::Ultra => "medium",
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

/// Framerate preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framerate {
    Fps15,
    Fps30,
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
    Native,
    Res1080p,
    Res720p,
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
    F8,
    F9,
    F10,
    CtrlShiftS,
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
    pub quality: Quality,
    pub framerate: Framerate,
    pub resolution: Resolution,
    pub buffer_seconds: u64,
    pub capture_audio: bool,
    pub audio_device: Option<String>,
    pub output_dir: PathBuf,
    pub hotkey: HotkeyPreset,
}

impl Default for Settings {
    fn default() -> Self {
        let output_dir = dirs::video_dir()
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
            .join("LiteClip");

        Self {
            quality: Quality::Medium,
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
