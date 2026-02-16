//! Platform Abstraction Layer
//!
//! Hidden HWND for hotkeys and system tray integration.

use anyhow::Result;
use crossbeam::channel::{Receiver, Sender};

pub mod hotkeys;
pub mod msg_loop;
pub mod tray;

/// Commands that can be sent to the platform thread
#[derive(Debug, Clone)]
pub enum PlatformCommand {
    /// Re-register hotkeys with new configuration
    ReRegisterHotkeys(HotkeyConfig),
}

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

/// Tray menu events
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    /// Open settings window
    OpenSettings,
    /// Save current clip
    SaveClip,
    /// Toggle recording on/off
    ToggleRecording,
    /// Exit the application
    Exit,
}

/// Application events from platform layer
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppEvent {
    /// Hotkey event with specific action
    Hotkey(HotkeyAction),
    /// Tray menu event
    Tray(TrayEvent),
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

/// Platform handle containing the thread handle and command sender
pub struct PlatformHandle {
    /// Thread handle for the platform message loop (Option allows taking ownership for join)
    thread: std::sync::Mutex<Option<std::thread::JoinHandle<()>>>,
    /// Command sender for sending commands to the platform thread
    pub command_tx: Sender<PlatformCommand>,
}

impl PlatformHandle {
    /// Create a new PlatformHandle
    pub fn new(thread: std::thread::JoinHandle<()>, command_tx: Sender<PlatformCommand>) -> Self {
        Self {
            thread: std::sync::Mutex::new(Some(thread)),
            command_tx,
        }
    }

    /// Send a command to the platform thread
    pub fn send_command(&self, cmd: PlatformCommand) -> Result<()> {
        self.command_tx
            .send(cmd)
            .map_err(|_| anyhow::anyhow!("Platform thread disconnected"))
    }

    /// Re-register hotkeys with a new configuration
    pub fn re_register_hotkeys(&self, config: HotkeyConfig) -> Result<()> {
        self.send_command(PlatformCommand::ReRegisterHotkeys(config))
    }

    /// Join the platform thread, waiting for it to complete
    /// Returns an error if the thread has already been joined
    pub fn join(&self) -> Result<()> {
        let mut guard = self
            .thread
            .lock()
            .map_err(|_| anyhow::anyhow!("PlatformHandle thread mutex poisoned"))?;

        if let Some(handle) = guard.take() {
            handle
                .join()
                .map_err(|_| anyhow::anyhow!("Platform thread panicked"))
        } else {
            Err(anyhow::anyhow!("Platform thread already joined"))
        }
    }
}

/// Spawn the platform message loop thread with hotkey configuration
///
/// Returns a [`PlatformHandle`] containing the thread handle and command sender,
/// along with a receiver for [`AppEvent`]s from the platform thread.
pub fn spawn_platform_thread(
    hotkey_config: HotkeyConfig,
) -> Result<(PlatformHandle, Receiver<AppEvent>)> {
    msg_loop::spawn_platform_thread(hotkey_config)
}
