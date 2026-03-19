//! Hidden-HWND Message Loop — hotkeys and tray.
//!
//! Creates a minimal hidden Win32 window for registering and receiving
//! `WM_HOTKEY` messages. Also manages the system tray icon using tray-icon.

use super::{AppEvent, HotkeyAction, HotkeyConfig, PlatformCommand, PlatformHandle};
use anyhow::{Context, Result};
use crossbeam::channel::{Receiver, Sender};
use tracing::{debug, error, info, trace};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, PeekMessageW, PostQuitMessage,
    RegisterClassW, TranslateMessage, CS_HREDRAW, CS_VREDRAW, HMENU, MSG, PM_NOREMOVE, PM_REMOVE,
    WM_DESTROY, WM_HOTKEY, WM_QUIT, WNDCLASSW, WS_EX_NOACTIVATE, WS_OVERLAPPED,
};

const CLASS_NAME: &str = "LiteClipReplay_HotkeyWindow";

/// Hotkey ID constants.
const HOTKEY_ID_SAVE_CLIP: i32 = 1000;
const HOTKEY_ID_TOGGLE_RECORDING: i32 = 1001;
const HOTKEY_ID_SCREENSHOT: i32 = 1002;
const HOTKEY_ID_OPEN_GALLERY: i32 = 1003;
const IDLE_LOOP_SLEEP_MS: u64 = 8;

/// Spawn the platform thread (hotkeys and tray).
pub fn spawn_platform_thread(
    hotkey_config: HotkeyConfig,
) -> Result<(PlatformHandle, Receiver<AppEvent>)> {
    let (event_tx, event_rx) = crossbeam::channel::unbounded::<AppEvent>();
    let (command_tx, command_rx) = crossbeam::channel::unbounded::<PlatformCommand>();

    let handle = std::thread::spawn(move || {
        if let Err(e) = run_platform_loop(event_tx, command_rx, hotkey_config) {
            error!("Platform message loop error: {}", e);
        }
    });

    Ok((PlatformHandle::new(handle, command_tx), event_rx))
}

/// Run the platform message loop (hotkeys + tray).
fn run_platform_loop(
    event_tx: Sender<AppEvent>,
    command_rx: Receiver<PlatformCommand>,
    hotkey_config: HotkeyConfig,
) -> Result<()> {
    debug!("Starting platform message loop (hotkeys + tray)");

    let hwnd = create_hidden_window()?;

    if let Err(e) = super::hotkeys::register_hotkeys(hwnd, &hotkey_config) {
        error!("Failed to register hotkeys: {}", e);
    }

    debug!("Hidden hotkey window created ({:?})", hwnd);

    // Create tray manager
    let mut tray_manager = match super::tray::TrayManager::new(event_tx.clone()) {
        Ok(tm) => {
            info!("Tray icon created successfully");
            Some(tm)
        }
        Err(e) => {
            error!("Failed to create tray icon: {}", e);
            None
        }
    };

    let mut msg = MSG::default();
    unsafe {
        loop {
            // Drain commands before blocking.
            while let Ok(cmd) = command_rx.try_recv() {
                match cmd {
                    PlatformCommand::ReRegisterHotkeys(new_cfg) => {
                        info!(
                            "Re-registering hotkeys: save={} toggle={} screenshot={} gallery={}",
                            new_cfg.save_clip,
                            new_cfg.toggle_recording,
                            new_cfg.screenshot,
                            new_cfg.open_gallery
                        );
                        if let Err(e) = super::hotkeys::unregister_all_hotkeys(hwnd) {
                            error!("Unregister hotkeys: {e}");
                        }
                        if let Err(e) = super::hotkeys::register_hotkeys(hwnd, &new_cfg) {
                            error!("Register hotkeys: {e}");
                        } else {
                            info!("Hotkeys re-registered");
                        }
                    }
                    PlatformCommand::UpdateRecordingState(_recording) => {
                        // Recording state updates are no longer needed for the tray menu
                        // since we removed the start/stop recording buttons
                    }
                    PlatformCommand::Quit => {
                        info!("Platform: Quit received, posting WM_QUIT");
                        PostQuitMessage(0);
                    }
                }
            }

            // Poll tray events - do this EVERY iteration, not just when no messages
            if let Some(ref mut tray) = tray_manager {
                tray.poll_events();
            }

            // Process ALL pending messages before potentially waiting
            // This ensures tray events (which come via Windows messages) are handled promptly
            let mut processed_any = false;
            while PeekMessageW(&mut msg, HWND::default(), 0, 0, PM_REMOVE).as_bool() {
                processed_any = true;
                if msg.message == WM_QUIT {
                    info!("WM_QUIT received in message loop");
                    break;
                }
                if msg.message == WM_HOTKEY {
                    let id = msg.wParam.0 as i32;
                    if let Some(action) = hotkey_id_to_action(id) {
                        trace!("WM_HOTKEY id={} -> {:?}", id, action);
                        if event_tx.send(AppEvent::Hotkey(action)).is_err() {
                            break;
                        }
                    }
                } else {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }

            // Check for WM_QUIT after processing batch
            if msg.message == WM_QUIT {
                break;
            }

            // Only sleep if no messages were processed AND no commands pending
            if !processed_any && command_rx.is_empty() {
                // Use a short wait, but check if messages are pending first
                // Peek with PM_NOREMOVE to check without removing
                if !PeekMessageW(&mut msg, HWND::default(), 0, 0, PM_NOREMOVE).as_bool() {
                    std::thread::sleep(std::time::Duration::from_millis(IDLE_LOOP_SLEEP_MS));
                }
            }
        }
    }

    if let Err(e) = super::hotkeys::unregister_all_hotkeys(hwnd) {
        error!("Unregister hotkeys on exit: {e}");
    }

    info!("Platform message loop exited");
    Ok(())
}

/// Create a minimal hidden window for receiving `WM_HOTKEY`.
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

    unsafe { RegisterClassW(&wndclass) };

    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_NOACTIVATE,
            windows::core::PCWSTR(class_name.as_ptr()),
            windows::core::PCWSTR::null(),
            WS_OVERLAPPED,
            -1000,
            -1000,
            0,
            0,
            HWND::default(),
            HMENU::default(),
            hinstance,
            None,
        )?
    };

    debug!("Hidden hotkey window: {:?}", hwnd);
    Ok(hwnd)
}

extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_DESTROY {
        unsafe { PostQuitMessage(0) };
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn hotkey_id_to_action(id: i32) -> Option<HotkeyAction> {
    match id {
        HOTKEY_ID_SAVE_CLIP => Some(HotkeyAction::SaveClip),
        HOTKEY_ID_TOGGLE_RECORDING => Some(HotkeyAction::ToggleRecording),
        HOTKEY_ID_SCREENSHOT => Some(HotkeyAction::Screenshot),
        HOTKEY_ID_OPEN_GALLERY => Some(HotkeyAction::OpenGallery),
        _ => {
            debug!("WM_HOTKEY with unknown id={}", id);
            None
        }
    }
}

/// Parse hotkey string (e.g. "Alt+F9") into modifiers + virtual key code.
pub fn parse_hotkey(hotkey: &str) -> Result<(HOT_KEY_MODIFIERS, u32)> {
    use crate::hotkey_parse::{
        parse_hotkey_components, MOD_ALT_BIT, MOD_CTRL_BIT, MOD_SHIFT_BIT, MOD_WIN_BIT,
    };

    let (bits, key) = parse_hotkey_components(hotkey).map_err(|e| anyhow::anyhow!(e))?;

    let mut modifiers = HOT_KEY_MODIFIERS(0);
    if bits & MOD_ALT_BIT != 0 {
        modifiers.0 |= MOD_ALT.0;
    }
    if bits & MOD_CTRL_BIT != 0 {
        modifiers.0 |= MOD_CONTROL.0;
    }
    if bits & MOD_SHIFT_BIT != 0 {
        modifiers.0 |= MOD_SHIFT.0;
    }
    if bits & MOD_WIN_BIT != 0 {
        modifiers.0 |= MOD_WIN.0;
    }

    trace!("Parsed '{}' -> mods={:?} key={}", hotkey, modifiers, key);
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

    #[test]
    fn test_parse_hotkey_rejects_unknown_token() {
        assert!(parse_hotkey("Alt+Mouse4").is_err());
    }

    #[test]
    fn test_parse_hotkey_rejects_multiple_keys() {
        assert!(parse_hotkey("Alt+F9+F10").is_err());
    }

    #[test]
    fn test_parse_hotkey_requires_modifier() {
        assert!(parse_hotkey("F9").is_err());
    }

    #[test]
    fn test_parse_hotkey_rejects_empty_token() {
        assert!(parse_hotkey("Alt++F9").is_err());
    }
}
