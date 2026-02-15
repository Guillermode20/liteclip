//! Platform Abstraction Layer
//!
//! Hidden HWND for hotkeys and system tray integration.

use anyhow::Result;
use crossbeam::channel::Receiver;

pub mod hotkeys;
pub mod msg_loop;

/// Hotkey actions that can be triggered by global hotkeys
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    /// Save clip hotkey pressed
    SaveClip,
    /// Toggle recording hotkey pressed
    ToggleRecording,
    /// Screenshot hotkey pressed
    Screenshot,
    /// Open gallery hotkey pressed
    OpenGallery,
}

/// Application events from platform layer
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppEvent {
    /// Hotkey event with specific action
    Hotkey(HotkeyAction),
    /// Quit application
    Quit,
}

/// Hotkey configuration for registration
#[derive(Debug, Clone)]
pub struct HotkeyConfig {
    /// Hotkey for saving clips (e.g., "Alt+F9")
    pub save_clip: String,
    /// Hotkey for toggling recording
    pub toggle_recording: String,
    /// Hotkey for screenshots
    pub screenshot: String,
    /// Hotkey for opening gallery
    pub open_gallery: String,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            save_clip: "Alt+F9".to_string(),
            toggle_recording: "Alt+F10".to_string(),
            screenshot: "Alt+F8".to_string(),
            open_gallery: "Ctrl+Shift+S".to_string(),
        }
    }
}

/// Spawn the platform message loop thread with hotkey configuration
pub fn spawn_platform_thread(
    hotkey_config: HotkeyConfig,
) -> Result<(std::thread::JoinHandle<()>, Receiver<AppEvent>)> {
    msg_loop::spawn_platform_thread(hotkey_config)
}
