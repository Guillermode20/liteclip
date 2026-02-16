//! Hotkey Registration and Management
//!
//! Win32 RegisterHotKey API wrapper for global hotkeys.

use super::HotkeyConfig;
use anyhow::{Context, Result};
use tracing::{debug, error, info, trace};
use windows::Win32::Foundation::{GetLastError, HWND};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS,
};

/// Windows error code for "Hot key is already registered"
const ERROR_HOTKEY_ALREADY_REGISTERED: u32 = 1409;

/// Hotkey definition
#[derive(Debug, Clone, Copy)]
pub struct Hotkey {
    /// Modifier flags (Alt, Ctrl, Shift, Win) using HOTKEYF_* constants
    pub modifiers: HOT_KEY_MODIFIERS,
    /// Virtual key code (e.g., 0x78 for F9)
    pub key: u32,
    /// Unique hotkey ID
    pub id: i32,
}

impl Hotkey {
    /// Create a new hotkey definition
    pub fn new(modifiers: HOT_KEY_MODIFIERS, key: u32, id: i32) -> Self {
        Self { modifiers, key, id }
    }

    /// Parse hotkey from string (e.g., "Alt+F9")
    pub fn from_str(s: &str, id: i32) -> Result<Self> {
        let (modifiers, key) = super::msg_loop::parse_hotkey(s)?;
        Ok(Self::new(modifiers, key, id))
    }
}

/// Hotkey ID constants - must match msg_loop.rs
const HOTKEY_ID_SAVE_CLIP: i32 = 1000;
const HOTKEY_ID_TOGGLE_RECORDING: i32 = 1001;
const HOTKEY_ID_SCREENSHOT: i32 = 1002;
const HOTKEY_ID_OPEN_GALLERY: i32 = 1003;

/// Register all hotkeys from configuration
///
/// Registers global hotkeys using Win32 RegisterHotKey API.
/// Hotkeys are registered with the hidden window as the target.
/// Logs errors but continues registering other hotkeys on failure.
pub fn register_hotkeys(hwnd: HWND, config: &HotkeyConfig) -> Result<()> {
    info!("Registering hotkeys...");

    // Register save clip hotkey (default: Alt+F9)
    if let Err(e) = register_single_hotkey(hwnd, HOTKEY_ID_SAVE_CLIP, &config.save_clip) {
        error!(
            "Failed to register save clip hotkey '{}': {}",
            config.save_clip, e
        );
    } else {
        debug!("Registered save clip hotkey: {}", config.save_clip);
    }

    // Register toggle recording hotkey (default: Alt+F10)
    if let Err(e) =
        register_single_hotkey(hwnd, HOTKEY_ID_TOGGLE_RECORDING, &config.toggle_recording)
    {
        error!(
            "Failed to register toggle recording hotkey '{}': {}",
            config.toggle_recording, e
        );
    } else {
        debug!(
            "Registered toggle recording hotkey: {}",
            config.toggle_recording
        );
    }

    // Register screenshot hotkey (default: Alt+F8)
    if let Err(e) = register_single_hotkey(hwnd, HOTKEY_ID_SCREENSHOT, &config.screenshot) {
        error!(
            "Failed to register screenshot hotkey '{}': {}",
            config.screenshot, e
        );
    } else {
        debug!("Registered screenshot hotkey: {}", config.screenshot);
    }

    // Register open gallery hotkey (default: Ctrl+Shift+S)
    if let Err(e) = register_single_hotkey(hwnd, HOTKEY_ID_OPEN_GALLERY, &config.open_gallery) {
        error!(
            "Failed to register open gallery hotkey '{}': {}",
            config.open_gallery, e
        );
    } else {
        debug!("Registered open gallery hotkey: {}", config.open_gallery);
    }

    info!("Hotkey registration complete");
    Ok(())
}

/// Register a single hotkey
///
/// Parses the hotkey string and calls Win32 RegisterHotKey
fn register_single_hotkey(hwnd: HWND, id: i32, hotkey_str: &str) -> Result<()> {
    let hotkey = Hotkey::from_str(hotkey_str, id)?;

    unsafe {
        if RegisterHotKey(hwnd, hotkey.id, hotkey.modifiers, hotkey.key).is_err() {
            let err = GetLastError();
            let code = err.0;
            let hint = if code == ERROR_HOTKEY_ALREADY_REGISTERED {
                " (another app has this hotkey - try different keys in config)"
            } else {
                ""
            };
            anyhow::bail!(
                "RegisterHotKey failed for hotkey '{}': Windows error {} (0x{:x}){}",
                hotkey_str,
                code,
                code,
                hint
            );
        }
    }

    trace!(
        "Registered hotkey: id={}, modifiers={:?}, key={:#x}",
        hotkey.id,
        hotkey.modifiers,
        hotkey.key
    );

    Ok(())
}

/// Unregister a single hotkey
///
/// Calls Win32 UnregisterHotKey for the given hotkey ID
pub fn unregister_hotkey(hwnd: HWND, id: i32) -> Result<()> {
    unsafe {
        UnregisterHotKey(hwnd, id)
            .ok()
            .context(format!("UnregisterHotKey failed for id={}", id))?;
    }

    trace!("Unregistered hotkey id={}", id);
    Ok(())
}

/// Unregister all hotkeys
///
/// Unregisters all known hotkey IDs. Logs errors but continues on failure.
pub fn unregister_all_hotkeys(hwnd: HWND) -> Result<()> {
    info!("Unregistering all hotkeys...");

    let hotkey_ids = [
        HOTKEY_ID_SAVE_CLIP,
        HOTKEY_ID_TOGGLE_RECORDING,
        HOTKEY_ID_SCREENSHOT,
        HOTKEY_ID_OPEN_GALLERY,
    ];

    for id in &hotkey_ids {
        if let Err(e) = unregister_hotkey(hwnd, *id) {
            error!("Failed to unregister hotkey id={}: {}", id, e);
        }
    }

    info!("Hotkey unregistration complete");
    Ok(())
}

/// Hotkey manager for registration/unregistration
///
/// Provides a higher-level interface for managing hotkeys.
pub struct HotkeyManager {
    hwnd: HWND,
    registered_ids: Vec<i32>,
}

impl HotkeyManager {
    /// Create new hotkey manager
    pub fn new(hwnd: HWND) -> Self {
        Self {
            hwnd,
            registered_ids: Vec::new(),
        }
    }

    /// Register a hotkey
    pub fn register(&mut self, hotkey: Hotkey) -> Result<()> {
        unsafe {
            RegisterHotKey(self.hwnd, hotkey.id, hotkey.modifiers, hotkey.key)
                .ok()
                .context("Failed to register hotkey")?;
        }

        debug!(
            "Registered hotkey id={} modifiers={:?} key={}",
            hotkey.id, hotkey.modifiers, hotkey.key
        );

        self.registered_ids.push(hotkey.id);
        Ok(())
    }

    /// Unregister a hotkey by ID
    pub fn unregister(&mut self, id: i32) -> Result<()> {
        unsafe {
            UnregisterHotKey(self.hwnd, id)
                .ok()
                .context("Failed to unregister hotkey")?;
        }

        self.registered_ids.retain(|&h| h != id);
        debug!("Unregistered hotkey id={}", id);
        Ok(())
    }

    /// Unregister all hotkeys
    pub fn unregister_all(&mut self) -> Result<()> {
        for id in &self.registered_ids {
            unsafe {
                let _ = UnregisterHotKey(self.hwnd, *id);
            }
        }
        self.registered_ids.clear();
        info!("All hotkeys unregistered");
        Ok(())
    }
}

impl Drop for HotkeyManager {
    fn drop(&mut self) {
        if let Err(e) = self.unregister_all() {
            error!("Failed to unregister hotkeys on drop: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hotkey_creation() {
        let modifiers = HOT_KEY_MODIFIERS(1);
        let hotkey = Hotkey::new(modifiers, 0x78, 1000);
        assert_eq!(hotkey.id, 1000);
        assert_eq!(hotkey.key, 0x78);
        assert_eq!(hotkey.modifiers.0, 1);
    }

    #[test]
    fn test_hotkey_from_str() {
        let hotkey = Hotkey::from_str("Alt+F9", 1000).unwrap();
        assert_eq!(hotkey.id, 1000);
        assert_eq!(hotkey.key, 0x78); // VK_F9
        assert!(hotkey.modifiers.0 > 0); // Should have Alt modifier
    }
}
