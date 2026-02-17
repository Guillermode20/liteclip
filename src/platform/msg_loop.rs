//! Hidden HWND Message Loop
//!
//! Dedicated thread with GetMessage/DispatchMessage pump for hotkeys and tray.

use super::{AppEvent, HotkeyAction, HotkeyConfig, PlatformCommand, PlatformHandle};
use super::tray::{TrayManager, WM_TRAY_CALLBACK};
use anyhow::{Context, Result};
use crossbeam::channel::{Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{debug, error, info, trace};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, GetWindowLongPtrW,
    PostQuitMessage, RegisterClassW, SetWindowLongPtrW, TranslateMessage, CS_HREDRAW,
    CS_VREDRAW, GWLP_USERDATA, HMENU, MSG, WM_COMMAND, WM_DESTROY, WM_HOTKEY, WNDCLASSW,
    WS_EX_NOACTIVATE, WS_OVERLAPPED,
};

const CLASS_NAME: &str = "LiteClipReplay_HiddenWindow";

/// Shared state accessible from window_proc via GWLP_USERDATA
struct WndProcState {
    tray_manager: Option<TrayManager>,
    is_recording: AtomicBool,
    event_tx: Sender<AppEvent>,
}

/// Hotkey ID constants - unique IDs for each hotkey
const HOTKEY_ID_SAVE_CLIP: i32 = 1000;
const HOTKEY_ID_TOGGLE_RECORDING: i32 = 1001;
const HOTKEY_ID_SCREENSHOT: i32 = 1002;
const HOTKEY_ID_OPEN_GALLERY: i32 = 1003;

/// Spawn the platform thread with message loop
///
/// Creates a hidden window, registers hotkeys and tray icon, and runs the Win32 message pump
/// in a dedicated thread. Events are sent to the main thread via crossbeam channel.
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

    let platform_handle = PlatformHandle::new(handle, command_tx);

    Ok((platform_handle, event_rx))
}

/// Run the platform message loop
///
/// Creates hidden window, registers hotkeys and tray, and processes Win32 messages
fn run_platform_loop(
    event_tx: Sender<AppEvent>,
    command_rx: Receiver<PlatformCommand>,
    hotkey_config: HotkeyConfig,
) -> Result<()> {
    debug!("Starting platform message loop");

    let hwnd = create_hidden_window()?;

    // Create tray manager
    let mut tray_manager: Option<TrayManager> = None;
    match TrayManager::new(hwnd) {
        Ok(mut tm) => {
            if let Err(e) = tm.add_icon() {
                error!("Failed to add tray icon: {}", e);
            } else {
                info!("System tray icon initialized");
                tray_manager = Some(tm);
            }
        }
        Err(e) => {
            error!("Failed to create tray manager: {}", e);
        }
    }

    // Register hotkeys from config
    if let Err(e) = super::hotkeys::register_hotkeys(hwnd, &hotkey_config) {
        error!("Failed to register hotkeys: {}", e);
    }

    debug!(
        "Hidden window created (hwnd: {:?}), hotkeys and tray registered",
        hwnd
    );

    // Shared state for window_proc to access tray manager
    let wnd_state = Box::new(WndProcState {
        tray_manager,
        is_recording: AtomicBool::new(false),
        event_tx: event_tx.clone(),
    });
    let wnd_state_ptr = Box::into_raw(wnd_state);

    // SAFETY: Store pointer in GWLP_USERDATA so window_proc can access tray state
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, wnd_state_ptr as isize);
    }

    let mut msg = MSG::default();
    unsafe {
        // Message pump - GetMessageW blocks until a message is available
        loop {
            // Check for commands from the main thread (non-blocking)
            if let Ok(cmd) = command_rx.try_recv() {
                let state = &*wnd_state_ptr;
                match cmd {
                    PlatformCommand::ReRegisterHotkeys(new_config) => {
                        info!("Re-registering hotkeys with new configuration");
                        if let Err(e) = super::hotkeys::unregister_all_hotkeys(hwnd) {
                            error!("Failed to unregister old hotkeys: {}", e);
                        }
                        if let Err(e) = super::hotkeys::register_hotkeys(hwnd, &new_config) {
                            error!("Failed to register new hotkeys: {}", e);
                        } else {
                            info!("Hotkeys re-registered successfully");
                        }
                    }
                    PlatformCommand::UpdateRecordingState(recording) => {
                        state.is_recording.store(recording, Ordering::Relaxed);
                    }
                    PlatformCommand::ShowNotification(title, message) => {
                        if let Some(ref tray) = state.tray_manager {
                            if let Err(e) = tray.show_notification(&title, &message) {
                                error!("Failed to show notification: {}", e);
                            }
                        }
                    }
                }
            }

            let result = GetMessageW(&mut msg, HWND::default(), 0, 0);
            if !result.as_bool() {
                break; // WM_QUIT received
            }

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

    // Cleanup - clear userdata, reclaim Box, drop tray manager
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        drop(Box::from_raw(wnd_state_ptr));
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
            -1000, // Off-screen position
            -1000,
            0,
            0,
            HWND::default(), // Regular window (not message-only) - required for tray menu focus
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
/// Handles WM_DESTROY, WM_TRAY_CALLBACK (sent by Shell), and WM_COMMAND (from TrackPopupMenu).
/// Tray state is accessed via GWLP_USERDATA pointer.
extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        WM_TRAY_CALLBACK | WM_COMMAND => {
            // SAFETY: WndProcState pointer is set before message pump starts and cleared after.
            // WM_TRAY_CALLBACK is sent (not posted) by Shell, so it only arrives here.
            // WM_COMMAND from TrackPopupMenu is also sent directly to the window proc.
            unsafe {
                let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const WndProcState;
                if !ptr.is_null() {
                    let state = &*ptr;
                    if let Some(ref tray) = state.tray_manager {
                        let recording = state.is_recording.load(Ordering::Relaxed);
                        if tray.handle_message(msg, wparam, lparam, recording, &state.event_tx) {
                            return LRESULT(0);
                        }
                    }
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
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
            "Alt" => modifiers.0 |= MOD_ALT.0,
            "Ctrl" | "Control" => modifiers.0 |= MOD_CONTROL.0,
            "Shift" => modifiers.0 |= MOD_SHIFT.0,
            "Win" => modifiers.0 |= MOD_WIN.0,
            _ => {
                // Parse function keys (F1-F24)
                if part.len() >= 2 && part.starts_with('F') {
                    if let Ok(fnum) = part[1..].parse::<u32>() {
                        if (1..=24).contains(&fnum) {
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
