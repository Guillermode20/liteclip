use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tracing::{debug, trace, warn};

use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Threading::{
    GetProcessId, OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsWindowVisible,
};

const KNOWN_GAMES: &[&str] = &[
    "valorant",
    "cs2",
    "csgo",
    "fortnite",
    "fortniteclient",
    "apex",
    "r5apex",
    "dota2",
    "dota",
    "league of legends",
    "lol",
    "leagueclient",
    "overwatch",
    "overwatch2",
    "minecraft",
    "javaw",
    "java",
    "gta5",
    "gtaiv",
    "gtav",
    "eldenring",
    "wow",
    "world of warcraft",
    "destiny2",
    "destiny",
    "warframe",
    "pubg",
    "tslg",
    "thecycle",
    "caldera",
    "roblox",
    "rbxfpsunlocker",
    "csgo",
    "counter-strike",
    "halo",
    "haloinfinite",
    "starfield",
    "cyberpunk2077",
    "cyberpunk",
    "reddeadredemption2",
    "rdr2",
    "bg3",
    "baldursgate3",
    "diablo4",
    "diablo iv",
    "pathofexile",
    "poegame",
    "lostark",
    "newworld",
    "gw2",
    "guildwars2",
    "ffxiv",
    "ffxiv_dx11",
    "final fantasy xiv",
    "eso",
    "elderscrollsonline",
    "rocketleague",
    "rl",
    "valorant",
    "fallguys",
    "amongus",
    "battlefield",
    "bf2042",
    "bfv",
    "callofduty",
    "cod",
    "mw2",
    "mw3",
    "warzone",
    "blackops",
    "sekiro",
    "darksouls3",
    "darksouls",
    "monsterhunter",
    "mhrise",
    "mhw",
    "streetfighter6",
    "tekken8",
    "tekken7",
    "dbfz",
    "guiltygear",
    "lol",
    "hearthstone",
    "starcraft",
    "sc2",
    "ageofempires",
    "aoe4",
    "aoe2",
    "civ6",
    "civilizationvi",
    "totalwar",
    "warhammer",
    "phasmophobia",
    "lethalcompany",
    "sons of the forest",
    "valheim",
    "terraria",
    "stardewvalley",
    "subnautica",
    "factorio",
    "satisfactory",
    "no man's sky",
    "sekiro",
    "hogwarts legacy",
    "hogwartslegacy",
];

#[derive(Debug, Clone)]
pub struct DetectedApp {
    pub name: String,
    pub folder_name: String,
    pub is_game: bool,
    pub is_fullscreen: bool,
}

pub struct GameDetector {
    detected: Arc<AtomicPtr<DetectedApp>>,
    running: Arc<AtomicBool>,
}

impl GameDetector {
    pub fn new() -> Self {
        let detected = Arc::new(AtomicPtr::new(Box::into_raw(Box::new(DetectedApp {
            name: String::new(),
            folder_name: String::new(),
            is_game: false,
            is_fullscreen: false,
        }))));

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
                    let ptr = Box::into_raw(Box::new(app));
                    let old_ptr = detected.swap(ptr, Ordering::SeqCst);
                    if !old_ptr.is_null() {
                        unsafe {
                            let _ = Box::from_raw(old_ptr);
                        }
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
        let ptr = self.detected.load(Ordering::SeqCst);
        if ptr.is_null() {
            DetectedApp {
                name: String::new(),
                folder_name: String::new(),
                is_game: false,
                is_fullscreen: false,
            }
        } else {
            unsafe { (*ptr).clone() }
        }
    }
}

impl Drop for GameDetector {
    fn drop(&mut self) {
        self.stop();
        let ptr = self.detected.load(Ordering::SeqCst);
        if !ptr.is_null() {
            unsafe {
                let _ = Box::from_raw(ptr);
            }
        }
    }
}

fn detect_foreground_app() -> Option<DetectedApp> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0 == 0 {
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
        let window_title = get_window_title(hwnd);
        let is_fullscreen = is_window_fullscreen(hwnd);

        let exe_lower = exe_name.to_lowercase();
        let title_lower = window_title.to_lowercase();

        let is_game = is_known_game(&exe_lower, &title_lower);

        let folder_name = sanitize_folder_name(&exe_name);

        Some(DetectedApp {
            name: exe_name,
            folder_name,
            is_game,
            is_fullscreen,
        })
    }
}

unsafe fn get_process_name(process_id: u32) -> Option<String> {
    let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id).ok()?;

    let mut buffer = [0u16; 260];
    let mut size = buffer.len() as u32;

    if QueryFullProcessImageNameW(handle, 0, &mut buffer, &mut size).is_err() {
        return None;
    }

    let path = String::from_utf16_lossy(&buffer[..size as usize]);
    let name = std::path::Path::new(&path)
        .file_stem()?
        .to_string_lossy()
        .to_string();

    Some(name)
}

unsafe fn get_window_title(hwnd: HWND) -> String {
    let len = GetWindowTextLengthW(hwnd);
    if len == 0 {
        return String::new();
    }

    let mut buffer = vec![0u16; (len + 1) as usize];
    GetWindowTextW(hwnd, &mut buffer);

    String::from_utf16_lossy(&buffer[..len as usize])
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

fn is_known_game(exe_name: &str, window_title: &str) -> bool {
    for game in KNOWN_GAMES {
        if exe_name.contains(game) {
            return true;
        }
    }

    for game in KNOWN_GAMES {
        if window_title.contains(game) {
            return true;
        }
    }

    false
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

    let trimmed: String = result.trim().to_string();
    if trimmed.is_empty() {
        "Unknown".to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_folder_name() {
        assert_eq!(sanitize_folder_name("Game: Name"), "Game_ Name");
        assert_eq!(sanitize_folder_name("Test<>Game"), "Test__Game");
        assert_eq!(sanitize_folder_name("Normal Game"), "Normal Game");
        assert_eq!(sanitize_folder_name(""), "Unknown");
    }

    #[test]
    fn test_is_known_game() {
        assert!(is_known_game("valorant", ""));
        assert!(is_known_game("cs2", ""));
        assert!(is_known_game("javaw", ""));
        assert!(is_known_game("", "Minecraft"));
        assert!(!is_known_game("notepad", ""));
        assert!(!is_known_game("chrome", "Google Chrome"));
    }
}
