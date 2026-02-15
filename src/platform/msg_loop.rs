//! Hidden HWND Message Loop
//!
//! Dedicated thread with GetMessage/DispatchMessage pump for hotkeys and tray.

use super::{AppEvent, HotkeyAction, HotkeyConfig};
use anyhow::{Context, Result};
use crossbeam::channel::{Receiver, Sender};
use tracing::{debug, error, info, trace};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, PostQuitMessage,
    RegisterClassW, TranslateMessage, CS_HREDRAW, CS_VREDRAW, HMENU, HWND_MESSAGE, MSG, WM_DESTROY,
    WM_HOTKEY, WNDCLASSW, WS_EX_NOACTIVATE, WS_OVERLAPPED,
};

const CLASS_NAME: &str = "LiteClipReplay_HiddenWindow";

/// Hotkey ID constants - unique IDs for each hotkey
const HOTKEY_ID_SAVE_CLIP: i32 = 1000;
const HOTKEY_ID_TOGGLE_RECORDING: i32 = 1001;
const HOTKEY_ID_SCREENSHOT: i32 = 1002;
const HOTKEY_ID_OPEN_GALLERY: i32 = 1003;

/// Spawn the platform thread with message loop
///
/// Creates a hidden window, registers hotkeys, and runs the Win32 message pump
/// in a dedicated thread. Events are sent to the main thread via crossbeam channel.
pub fn spawn_platform_thread(
    hotkey_config: HotkeyConfig,
) -> Result<(std::thread::JoinHandle<()>, Receiver<AppEvent>)> {
    let (event_tx, event_rx) = crossbeam::channel::unbounded::<AppEvent>();

    let handle = std::thread::spawn(move || {
        if let Err(e) = run_platform_loop(event_tx, hotkey_config) {
            error!("Platform message loop error: {}", e);
        }
    });

    Ok((handle, event_rx))
}

/// Run the platform message loop
///
/// Creates hidden window, registers hotkeys, and processes Win32 messages
fn run_platform_loop(event_tx: Sender<AppEvent>, hotkey_config: HotkeyConfig) -> Result<()> {
    info!("Starting platform message loop");

    let hwnd = create_hidden_window()?;

    // Register hotkeys from config
    if let Err(e) = super::hotkeys::register_hotkeys(hwnd, &hotkey_config) {
        error!("Failed to register hotkeys: {}", e);
    }

    info!(
        "Hidden window created (hwnd: {:?}), hotkeys registered",
        hwnd
    );

    let mut msg = MSG::default();
    unsafe {
        // Message pump - GetMessageW blocks until a message is available
        while GetMessageW(&mut msg, HWND::default(), 0, 0).as_bool() {
            match msg.message {
                WM_HOTKEY => {
                    let action = hotkey_id_to_action(msg.wParam.0 as i32);
                    trace!(
                        "WM_HOTKEY received: id={}, action={:?}",
                        msg.wParam.0,
                        action
                    );

                    if let Err(e) = event_tx.send(AppEvent::Hotkey(action)) {
                        error!("Failed to send hotkey event: {}", e);
                        // Channel closed - time to exit
                        break;
                    }
                }
                _ => {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
    }

    // Cleanup hotkeys before exit
    if let Err(e) = super::hotkeys::unregister_all_hotkeys(hwnd) {
        error!("Failed to unregister hotkeys: {}", e);
    }

    info!("Platform message loop exited");
    Ok(())
}

/// Create a hidden message-only window
///
/// Uses RegisterClassW + CreateWindowExW with HWND_MESSAGE parent
/// to create a window that doesn't appear in the taskbar or desktop.
fn create_hidden_window() -> Result<HWND> {
    let class_name: Vec<u16> = CLASS_NAME.encode_utf16().chain(Some(0)).collect();

    let hinstance = unsafe { GetModuleHandleW(None).context("Failed to get module handle")? };
    
    let wndclass = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: hinstance.into(),
        lpszClassName: windows::core::PCWSTR(class_name.as_ptr()),
        style: CS_HREDRAW | CS_VREDRAW,
        ..Default::default()
    };

    unsafe {
        RegisterClassW(&wndclass);
    }

    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_NOACTIVATE,
            windows::core::PCWSTR(class_name.as_ptr()),
            windows::core::PCWSTR::null(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            HWND_MESSAGE, // Message-only window - doesn't receive broadcast messages
            HMENU::default(),
            hinstance,
            None,
        )?
    };

    debug!("Hidden window created: {:?}", hwnd);
    Ok(hwnd)
}

/// Window procedure for the hidden window
///
/// Handles WM_DESTROY to post quit message, delegates others to DefWindowProcW
extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Map hotkey ID to action
fn hotkey_id_to_action(id: i32) -> HotkeyAction {
    match id {
        HOTKEY_ID_SAVE_CLIP => HotkeyAction::SaveClip,
        HOTKEY_ID_TOGGLE_RECORDING => HotkeyAction::ToggleRecording,
        HOTKEY_ID_SCREENSHOT => HotkeyAction::Screenshot,
        HOTKEY_ID_OPEN_GALLERY => HotkeyAction::OpenGallery,
        _ => HotkeyAction::SaveClip, // Default fallback
    }
}

/// Parse hotkey string (e.g., "Alt+F9") into modifiers and virtual key code
///
/// Returns (modifiers, key) where modifiers is a combination of MOD_* flags
/// and key is the Windows virtual key code.
pub fn parse_hotkey(hotkey: &str) -> Result<(HOT_KEY_MODIFIERS, u32)> {
    let parts: Vec<&str> = hotkey.split('+').map(|s| s.trim()).collect();

    let mut modifiers = HOT_KEY_MODIFIERS(0);
    let mut key = 0u32;

    for part in &parts {
        match *part {
            "Alt" => modifiers.0 |= MOD_ALT.0 as u32,
            "Ctrl" | "Control" => modifiers.0 |= MOD_CONTROL.0 as u32,
            "Shift" => modifiers.0 |= MOD_SHIFT.0 as u32,
            "Win" => modifiers.0 |= MOD_WIN.0 as u32,
            _ => {
                // Parse function keys (F1-F24)
                if part.len() >= 2 && part.starts_with('F') {
                    if let Ok(fnum) = part[1..].parse::<u32>() {
                        if fnum >= 1 && fnum <= 24 {
                            // VK_F1 = 0x70
                            key = 0x6F + fnum;
                        }
                    }
                } else if part.len() == 1 {
                    // Single character keys (A-Z, 0-9)
                    let ch = part.chars().next().unwrap().to_ascii_uppercase() as u32;
                    if (0x30..=0x39).contains(&ch) || (0x41..=0x5A).contains(&ch) {
                        key = ch;
                    }
                }
            }
        }
    }

    if key == 0 {
        anyhow::bail!("Could not parse hotkey: {}", hotkey);
    }

    trace!(
        "Parsed hotkey '{}' -> modifiers={:?}, key={}",
        hotkey,
        modifiers,
        key
    );
    Ok((modifiers, key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hotkey_alt_f9() {
        let (mods, key) = parse_hotkey("Alt+F9").unwrap();
        assert!(mods.0 > 0);
        assert_eq!(key, 0x78); // VK_F9
    }

    #[test]
    fn test_parse_hotkey_ctrl_shift_s() {
        let (mods, key) = parse_hotkey("Ctrl+Shift+S").unwrap();
        assert!(mods.0 > 0);
        assert_eq!(key, 0x53); // VK_S
    }
}
