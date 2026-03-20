use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::Duration;
use tracing::debug;

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible,
};

/// Information about a detected application (game or other).
///
/// Returned by [`GameDetector`] when querying the currently detected application.
#[derive(Debug, Clone, Default)]
pub struct DetectedApp {
    /// Display name of the detected application.
    pub name: String,
    /// Folder name for organizing saved clips.
    pub folder_name: String,
    /// Whether this is identified as a game.
    pub is_game: bool,
}

/// Game detector for identifying running games.
///
/// Periodically scans the foreground window and identifies if it's a game
/// based on known game executable names.
///
/// # Example
///
/// ```no_run
/// use liteclip_replay::detection::GameDetector;
///
/// let detector = GameDetector::new();
/// detector.start();
///
/// // Later, check detection result
/// let app = detector.get_detected_app();
/// if app.is_game {
///     println!("Playing: {}", app.name);
/// }
/// ```
pub struct GameDetector {
    detected: Arc<RwLock<DetectedApp>>,
    running: Arc<AtomicBool>,
}

impl Default for GameDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl GameDetector {
    pub fn new() -> Self {
        let detected = Arc::new(RwLock::new(DetectedApp {
            name: String::new(),
            folder_name: String::new(),
            is_game: false,
        }));

        Self {
            detected,
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn start(&self) {
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }

        let detected = self.detected.clone();
        let running = self.running.clone();

        thread::spawn(move || {
            debug!("Game detector thread started");

            while running.load(Ordering::SeqCst) {
                if let Some(app) = detect_foreground_app() {
                    if let Ok(mut g) = detected.write() {
                        *g = app;
                    }
                }

                thread::sleep(Duration::from_millis(500));
            }

            debug!("Game detector thread stopped");
        });
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    pub fn get_detected_app(&self) -> DetectedApp {
        self.detected.read().map(|g| g.clone()).unwrap_or_default()
    }
}

impl Drop for GameDetector {
    fn drop(&mut self) {
        self.stop();
    }
}

fn detect_foreground_app() -> Option<DetectedApp> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }

        if !IsWindowVisible(hwnd).as_bool() {
            return None;
        }

        let mut process_id: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut process_id));

        if process_id == 0 {
            return None;
        }

        let exe_name = get_process_name(process_id)?;
        let is_fullscreen = is_window_fullscreen(hwnd);

        let folder_name = sanitize_folder_name(&exe_name);

        Some(DetectedApp {
            name: exe_name,
            folder_name,
            is_game: is_fullscreen,
        })
    }
}

unsafe fn get_process_name(process_id: u32) -> Option<String> {
    let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id).ok()?;

    let mut buffer = [0u16; 260];
    let mut size = buffer.len() as u32;

    if QueryFullProcessImageNameW(
        handle,
        PROCESS_NAME_FORMAT(0),
        windows::core::PWSTR(buffer.as_mut_ptr()),
        &mut size,
    )
    .is_err()
    {
        return None;
    }

    let path = String::from_utf16_lossy(&buffer[..size as usize]);
    let name = std::path::Path::new(&path)
        .file_stem()?
        .to_string_lossy()
        .to_string();

    Some(name)
}

unsafe fn is_window_fullscreen(hwnd: HWND) -> bool {
    let mut window_rect = RECT::default();
    if GetWindowRect(hwnd, &mut window_rect).is_err() {
        return false;
    }

    let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
    let mut monitor_info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };

    if GetMonitorInfoW(monitor, &mut monitor_info).as_bool() {
        let monitor_rect = monitor_info.rcMonitor;

        let window_width = window_rect.right - window_rect.left;
        let window_height = window_rect.bottom - window_rect.top;
        let monitor_width = monitor_rect.right - monitor_rect.left;
        let monitor_height = monitor_rect.bottom - monitor_rect.top;

        window_width == monitor_width && window_height == monitor_height
    } else {
        false
    }
}

fn sanitize_folder_name(name: &str) -> String {
    let invalid_chars = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
    let mut result = String::with_capacity(name.len());

    for c in name.chars() {
        if invalid_chars.contains(&c) || c.is_control() {
            result.push('_');
        } else {
            result.push(c);
        }
    }

    if result.is_empty() {
        return "Unknown".to_string();
    }

    result.trim().to_string()
}
