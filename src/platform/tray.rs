//! System Tray Integration
//!
//! Creates a system tray icon with a context menu for accessing settings
//! and controlling the application.

use anyhow::{Context, Result};
use crossbeam::channel::Sender;
use tracing::{debug, error, info, trace};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIF_INFO, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, LoadIconW, PostMessageW,
    SetForegroundWindow, TrackPopupMenu, HMENU, IDI_APPLICATION, MF_SEPARATOR, MF_STRING,
    TPM_BOTTOMALIGN, TPM_LEFTALIGN, WM_APP, WM_COMMAND, WM_RBUTTONUP,
};

use super::AppEvent;

const TRAY_ICON_ID: u32 = 1;
pub const WM_TRAY_CALLBACK: u32 = WM_APP + 1;

/// Tray menu item IDs
const MENU_ITEM_SETTINGS: u32 = 1001;
const MENU_ITEM_SAVE_CLIP: u32 = 1002;
const MENU_ITEM_TOGGLE_RECORDING: u32 = 1003;
const MENU_ITEM_START_RECORDING: u32 = 1005;
const MENU_ITEM_STOP_RECORDING: u32 = 1006;
const MENU_ITEM_EXIT: u32 = 1004;

/// System tray manager
pub struct TrayManager {
    hwnd: HWND,
    is_visible: bool,
}

impl TrayManager {
    /// Create a new tray manager for the given window
    pub fn new(hwnd: HWND) -> Result<Self> {
        debug!("Tray manager created for hwnd: {:?}", hwnd);

        Ok(Self {
            hwnd,
            is_visible: false,
        })
    }

    /// Create and add the tray icon
    pub fn add_icon(&mut self) -> Result<()> {
        if self.is_visible {
            return Ok(());
        }

        // SAFETY: NOTIFYICONDATAW is properly initialized
        unsafe {
            // For system icons like IDI_APPLICATION, hInstance must be NULL
            let hicon = LoadIconW(
                windows::Win32::Foundation::HINSTANCE(std::ptr::null_mut()),
                IDI_APPLICATION
            ).context("Failed to load icon")?;

            let tooltip: Vec<u16> = "LiteClip Replay".encode_utf16().chain(Some(0)).collect();

            let mut nid = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: self.hwnd,
                uID: TRAY_ICON_ID,
                uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
                uCallbackMessage: WM_TRAY_CALLBACK,
                hIcon: hicon,
                ..Default::default()
            };

            // Set tooltip (copy first 127 characters + null terminator)
            let tooltip_len = std::cmp::min(tooltip.len().saturating_sub(1), 127);
            nid.szTip[..tooltip_len].copy_from_slice(&tooltip[..tooltip_len]);
            nid.szTip[tooltip_len] = 0;

            Shell_NotifyIconW(NIM_ADD, &nid)
                .ok()
                .context("Failed to add tray icon")?;
        }

        self.is_visible = true;
        info!("System tray icon added");

        Ok(())
    }

    /// Remove the tray icon
    pub fn remove_icon(&mut self) -> Result<()> {
        if !self.is_visible {
            return Ok(());
        }

        let nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: TRAY_ICON_ID,
            ..Default::default()
        };

        // SAFETY: NOTIFYICONDATAW is properly initialized
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
        }

        self.is_visible = false;
        debug!("System tray icon removed");

        Ok(())
    }

    /// Handle window messages for tray
    ///
    /// Returns true if the message was handled
    pub fn handle_message(
        &self,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        is_recording: bool,
        event_tx: &Sender<AppEvent>,
    ) -> bool {
        // Check if it's a tray callback message
        if msg == WM_TRAY_CALLBACK {
            self.handle_tray_callback(lparam, is_recording, event_tx);
            return true;
        }

        // Handle menu commands
        if msg == WM_COMMAND {
            let cmd_id = (wparam.0 & 0xFFFF) as u32; // LOWORD
            if self.handle_command(cmd_id, event_tx) {
                return true;
            }
        }

        false
    }

    /// Show the context menu at the specified screen coordinates
    pub fn show_menu(
        &self,
        x: i32,
        y: i32,
        is_recording: bool,
        _event_tx: &Sender<AppEvent>,
    ) -> Result<()> {
        // SAFETY: Menu operations are properly guarded
        unsafe {
            let hmenu = CreatePopupMenu().context("Failed to create popup menu")?;

            // Add menu items
            Self::append_menu_string(hmenu, MENU_ITEM_SETTINGS, "Settings")?;
            Self::append_menu_string(hmenu, MENU_ITEM_SAVE_CLIP, "Save Clip")?;

            // Add dynamic recording control
            if is_recording {
                Self::append_menu_string(hmenu, MENU_ITEM_STOP_RECORDING, "Stop Recording")?;
            } else {
                Self::append_menu_string(hmenu, MENU_ITEM_START_RECORDING, "Start Recording")?;
            }

            Self::append_menu_separator(hmenu)?;
            Self::append_menu_string(hmenu, MENU_ITEM_EXIT, "Exit")?;

            // Required for the menu to work properly
            let _ = SetForegroundWindow(self.hwnd);

            // Show the menu
            TrackPopupMenu(
                hmenu,
                TPM_LEFTALIGN | TPM_BOTTOMALIGN,
                x,
                y,
                0,
                self.hwnd,
                None,
            )
            .ok()
            .context("Failed to track popup menu")?;

            // Required by Windows API when using TrackPopupMenu
            PostMessageW(self.hwnd, 0, WPARAM(0), LPARAM(0)).ok();

            // Clean up menu
            DestroyMenu(hmenu).ok();
        }

        Ok(())
    }

    /// Handle WM_COMMAND from menu selection
    fn handle_command(&self, cmd_id: u32, event_tx: &Sender<AppEvent>) -> bool {
        match cmd_id {
            MENU_ITEM_SETTINGS => {
                trace!("Tray: Settings menu selected");
                if let Err(e) = event_tx.send(AppEvent::Tray(super::TrayEvent::OpenSettings)) {
                    error!("Failed to send settings event: {}", e);
                }
                true
            }
            MENU_ITEM_SAVE_CLIP => {
                trace!("Tray: Save Clip menu selected");
                if let Err(e) = event_tx.send(AppEvent::Tray(super::TrayEvent::SaveClip)) {
                    error!("Failed to send save clip event: {}", e);
                }
                true
            }
            MENU_ITEM_TOGGLE_RECORDING => {
                trace!("Tray: Toggle Recording menu selected");
                if let Err(e) = event_tx.send(AppEvent::Tray(super::TrayEvent::ToggleRecording)) {
                    error!("Failed to send toggle recording event: {}", e);
                }
                true
            }
            MENU_ITEM_START_RECORDING => {
                trace!("Tray: Start Recording menu selected");
                if let Err(e) = event_tx.send(AppEvent::Tray(super::TrayEvent::StartRecording)) {
                    error!("Failed to send start recording event: {}", e);
                }
                true
            }
            MENU_ITEM_STOP_RECORDING => {
                trace!("Tray: Stop Recording menu selected");
                if let Err(e) = event_tx.send(AppEvent::Tray(super::TrayEvent::StopRecording)) {
                    error!("Failed to send stop recording event: {}", e);
                }
                true
            }
            MENU_ITEM_EXIT => {
                trace!("Tray: Exit menu selected");
                if let Err(e) = event_tx.send(AppEvent::Tray(super::TrayEvent::Exit)) {
                    error!("Failed to send exit event: {}", e);
                }
                true
            }
            _ => false,
        }
    }

    /// Handle tray callback message (WM_LBUTTONUP, WM_RBUTTONUP)
    fn handle_tray_callback(&self, lparam: LPARAM, is_recording: bool, event_tx: &Sender<AppEvent>) -> bool {
        let msg = lparam.0 as u32;

        match msg {
            WM_RBUTTONUP => {
                // Right click - show context menu
                trace!("Tray: Right button clicked, showing menu");

                // Get cursor position
                let mut pt = windows::Win32::Foundation::POINT::default();
                unsafe {
                    GetCursorPos(&mut pt).ok();
                }

                if let Err(e) = self.show_menu(pt.x, pt.y, is_recording, event_tx) {
                    error!("Failed to show tray menu: {}", e);
                }
                true
            }
            _ => false,
        }
    }

    /// Append a menu item to the menu
    unsafe fn append_menu_string(hmenu: HMENU, id: u32, text: &str) -> Result<()> {
        let wide_text: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();

        AppendMenuW(
            hmenu,
            MF_STRING,
            id as usize,
            windows::core::PCWSTR(wide_text.as_ptr()),
        )
        .ok()
        .context("Failed to append menu item")?;

        Ok(())
    }

    /// Append a separator to the menu
    unsafe fn append_menu_separator(hmenu: HMENU) -> Result<()> {
        AppendMenuW(hmenu, MF_SEPARATOR, 0, windows::core::PCWSTR::null())
            .ok()
            .context("Failed to append menu separator")?;

        Ok(())
    }

    /// Show a balloon notification from the tray icon
    pub fn show_notification(&self, title: &str, message: &str) -> Result<()> {
        if !self.is_visible {
            return Ok(());
        }

        unsafe {
            let title_wide: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
            let message_wide: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();

            let mut nid = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: self.hwnd,
                uID: TRAY_ICON_ID,
                uFlags: NIF_INFO,
                szInfoTitle: [0u16; 64],
                szInfo: [0u16; 256],
                ..Default::default()
            };

            // Copy title (max 63 chars + null)
            let title_len = std::cmp::min(title_wide.len().saturating_sub(1), 63);
            nid.szInfoTitle[..title_len].copy_from_slice(&title_wide[..title_len]);
            nid.szInfoTitle[title_len] = 0;

            // Copy message (max 255 chars + null)
            let msg_len = std::cmp::min(message_wide.len().saturating_sub(1), 255);
            nid.szInfo[..msg_len].copy_from_slice(&message_wide[..msg_len]);
            nid.szInfo[msg_len] = 0;

            Shell_NotifyIconW(NIM_ADD, &nid).ok()
                .context("Failed to show notification")?;
        }

        Ok(())
    }
}

impl Drop for TrayManager {
    fn drop(&mut self) {
        let _ = self.remove_icon();
    }
}
