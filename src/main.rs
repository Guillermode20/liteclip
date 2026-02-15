//! LiteClip Recorder
//!
//! A lightweight screen recording application with a rolling replay buffer,
//! similar to Medal.tv or ShadowPlay. Built with Rust, eframe (egui), and FFmpeg.

mod gui;
mod recorder;
mod settings;

use eframe::egui;
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use log::{error, info, warn};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem},
    TrayIconBuilder,
};

use gui::LiteClipApp;
use recorder::Recorder;
use settings::HotkeyPreset;

/// Main entry point for the LiteClip application.
/// Initializes logging, shared state, hotkey manager, tray icon, and starts the eframe GUI loop.
fn main() -> eframe::Result<()> {
    init_logging();

    // --- Shared recorder state ---
    let recorder = Arc::new(Mutex::new(Recorder::new()));

    // --- Ctrl+C handler: ensure FFmpeg child is killed on forced exit ---
    {
        let recorder_for_ctrlc = recorder.clone();
        let _ = ctrlc::set_handler(move || {
            info!("Ctrl+C received — stopping recorder");
            if let Ok(mut rec) = recorder_for_ctrlc.lock() {
                rec.stop();
            }
            std::process::exit(0);
        });
    }

    // --- Register initial global hotkey ---
    let hotkey_manager = match GlobalHotKeyManager::new() {
        Ok(manager) => Some(manager),
        Err(e) => {
            error!(
                "Failed to initialize global hotkey manager: {}. Hotkeys will be disabled.",
                e
            );
            None
        }
    };
    let initial_hotkey = {
        let rec = recorder.lock().unwrap();
        rec.settings.hotkey
    };
    let (current_hotkey, current_hotkey_id, active_hotkey_preset) = match hotkey_manager.as_ref() {
        Some(manager) => match register_available_hotkey(manager, initial_hotkey) {
            Some((registered_hotkey, preset)) => {
                let registered_hotkey_id = registered_hotkey.id();
                info!("Initial hotkey registered: {}", preset.label());
                (Some(registered_hotkey), registered_hotkey_id, preset)
            }
            None => {
                error!("Failed to register any global hotkey; hotkey is temporarily disabled");
                (None, 0, initial_hotkey)
            }
        },
        None => {
            warn!("Global hotkey manager unavailable; hotkey is disabled");
            (None, 0, initial_hotkey)
        }
    };

    if active_hotkey_preset != initial_hotkey {
        if let Ok(mut rec) = recorder.lock() {
            rec.settings.hotkey = active_hotkey_preset;
            rec.settings.save();
        }
        warn!(
            "Preferred hotkey {} unavailable at startup; switched to {}",
            initial_hotkey.label(),
            active_hotkey_preset.label()
        );
    }

    let hotkey_id_shared = Arc::new(AtomicU32::new(current_hotkey_id));

    // --- System tray icon ---
    let tray_menu = Menu::new();
    let show_item = MenuItem::new("Show LiteClip Replay", true, None);
    let quit_item = MenuItem::new("Quit", true, None);
    let _ = tray_menu.append(&show_item);
    let _ = tray_menu.append(&quit_item);

    let show_item_id = show_item.id().clone();
    let quit_item_id = quit_item.id().clone();

    // Create a 32x32 tray icon: white recording dot inside a black circle.
    let icon = create_tray_icon_image();

    let _tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_tooltip("LiteClip Replay")
        .with_icon(icon)
        .build()
        .expect("Failed to create tray icon");

    // --- Shared flags for tray menu actions ---
    // A background thread polls MenuEvent (which works even when the window is hidden)
    // and sets these flags. The update() loop reads them.
    let tray_show_requested = Arc::new(AtomicBool::new(false));
    let egui_ctx_handle: Arc<Mutex<Option<egui::Context>>> = Arc::new(Mutex::new(None));
    let show_flag = tray_show_requested.clone();
    let egui_ctx_for_tray = egui_ctx_handle.clone();
    let recorder_for_quit = recorder.clone();

    // Spawn a dedicated thread to listen for tray menu events
    std::thread::spawn(move || {
        let menu_rx = MenuEvent::receiver();
        loop {
            if let Ok(event) = menu_rx.recv() {
                if event.id == show_item_id {
                    info!("Tray thread: Show requested");
                    show_flag.store(true, Ordering::SeqCst);
                    if let Ok(guard) = egui_ctx_for_tray.lock() {
                        if let Some(ctx) = guard.clone() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                            ctx.request_repaint();
                        }
                    }
                } else if event.id == quit_item_id {
                    info!("Tray thread: Quit — stopping recorder and exiting");
                    if let Ok(mut rec) = recorder_for_quit.lock() {
                        rec.stop();
                    }
                    std::process::exit(0);
                }
            }
        }
    });

    let recorder_for_hotkey = recorder.clone();
    let hotkey_id_for_thread = hotkey_id_shared.clone();
    std::thread::spawn(move || {
        let hotkey_rx = GlobalHotKeyEvent::receiver();
        loop {
            match hotkey_rx.recv() {
                Ok(event) => {
                    let active_hotkey_id = hotkey_id_for_thread.load(Ordering::SeqCst);
                    if active_hotkey_id != 0
                        && event.id == active_hotkey_id
                        && event.state == HotKeyState::Released
                    {
                        info!("Global hotkey triggered — saving clip");
                        if let Err(e) = Recorder::save_clip_auto_detached(&recorder_for_hotkey) {
                            error!("Hotkey save failed: {}", e);
                        }
                    }
                }
                Err(e) => {
                    error!("Global hotkey event receiver stopped: {}", e);
                    break;
                }
            }
        }
    });

    // Periodic recorder watchdog for unexpected FFmpeg exits and automatic recovery.
    let recorder_for_watchdog = recorder.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(2));
        match recorder_for_watchdog.lock() {
            Ok(mut rec) => rec.health_check_tick(),
            Err(_) => {
                error!("Recorder watchdog stopping: recorder lock poisoned");
                break;
            }
        }
    });

    let recorder_for_app = recorder.clone();
    let hotkey_id_for_app = hotkey_id_shared.clone();
    let egui_ctx_for_app = egui_ctx_handle.clone();

    // --- Window options ---
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("LiteClip Replay")
            .with_inner_size([340.0, 320.0])
            .with_min_inner_size([300.0, 260.0])
            .with_max_inner_size([500.0, 500.0])
            .with_always_on_top()
            .with_icon(create_window_icon_data()),
        ..Default::default()
    };

    eframe::run_native(
        "LiteClip Replay",
        options,
        Box::new(move |_cc| {
            Ok(Box::new(HotkeyWrapper {
                app: LiteClipApp::new(recorder_for_app),
                hotkey_manager,
                current_hotkey,
                current_hotkey_id,
                hotkey_id_shared: hotkey_id_for_app,
                tray_show_requested,
                egui_ctx_handle: egui_ctx_for_app,
            }))
        }),
    )
}

/// Wraps the LiteClipApp to also poll for global hotkey events,
/// tray icon menu events, and handle dynamic hotkey re-registration.
struct HotkeyWrapper {
    /// The actual LiteClip GUI application
    app: LiteClipApp,
    /// Manager for global hotkeys
    hotkey_manager: Option<GlobalHotKeyManager>,
    /// The currently registered hotkey configuration
    current_hotkey: Option<HotKey>,
    /// The unique ID of the currently registered hotkey
    current_hotkey_id: u32,
    /// Shared active hotkey id used by the listener thread.
    hotkey_id_shared: Arc<AtomicU32>,
    /// Set by background thread when "Show LiteClip" is clicked in tray
    tray_show_requested: Arc<AtomicBool>,
    /// Latest egui context for tray thread wake-up/show operations.
    egui_ctx_handle: Arc<Mutex<Option<egui::Context>>>,
}

impl eframe::App for HotkeyWrapper {
    /// Updates the application state every frame.
    /// Polls for hotkey events, tray flags, and delegates UI rendering to [`LiteClipApp`].
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if let Ok(mut handle) = self.egui_ctx_handle.lock() {
            *handle = Some(ctx.clone());
        }

        // --- Handle tray show ---
        if self.tray_show_requested.swap(false, Ordering::SeqCst) {
            info!("Tray: Restoring window");
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }

        // --- Handle close-to-tray ---
        if ctx.input(|i| i.viewport().close_requested()) {
            let minimize_to_tray = {
                if let Ok(rec) = self.app.recorder.try_lock() {
                    rec.settings.minimize_to_tray
                } else {
                    false
                }
            };

            if minimize_to_tray {
                // Cancel the close and minimize the window instead.
                // Using Minimized keeps the event loop alive (unlike Visible(false)
                // which kills it on Windows), so the tray Show button works.
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                info!("Window minimized to tray");
            }
        }

        // Check if the GUI requested a hotkey change
        if let Some(new_preset) = self.app.pending_hotkey.take() {
            if let Some(hotkey_manager) = self.hotkey_manager.as_ref() {
                let new_hotkey = create_hotkey(new_preset);
                if new_hotkey.id() != self.current_hotkey_id {
                    match hotkey_manager.register(new_hotkey) {
                        Ok(()) => {
                            if let Some(old_hotkey) = self.current_hotkey.take() {
                                let _ = hotkey_manager.unregister(old_hotkey);
                            }
                            info!("Hotkey changed to {}", new_preset.label());
                            self.current_hotkey = Some(new_hotkey);
                            self.current_hotkey_id = new_hotkey.id();
                            self.hotkey_id_shared
                                .store(self.current_hotkey_id, Ordering::SeqCst);
                            if let Ok(mut rec) = self.app.recorder.lock() {
                                rec.settings.hotkey = new_preset;
                                rec.settings.save();
                            }
                            self.app.status_message =
                                format!("Hotkey changed to {}", new_preset.label());
                            self.app.status_timer = 3.0;
                        }
                        Err(e) => {
                            error!("Failed to register hotkey {}: {}", new_preset.label(), e);
                            self.app.status_message = format!("Hotkey error: {}", e);
                            self.app.status_timer = 5.0;
                        }
                    }
                }
            } else {
                self.app.status_message = "Global hotkeys are unavailable on this system.".into();
                self.app.status_timer = 5.0;
            }
        }

        // Delegate to the actual app
        self.app.update(ctx, frame);

        // Keep the event loop alive even when hidden, so tray flags are checked
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }
}

/// Initializes the logging system using `simplelog`.
/// Logs are written to both the terminal and a file in the user's video directory.
fn init_logging() {
    use simplelog::*;
    use std::fs;

    // Place log file in the output directory (~/Videos/LiteClip/)
    let log_dir = dirs::video_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from(".")))
        .join("LiteClip");
    let _ = fs::create_dir_all(&log_dir);
    let log_path = log_dir.join("liteclip.log");

    let config = ConfigBuilder::new()
        .set_time_format_rfc3339()
        .set_target_level(LevelFilter::Off)
        .set_thread_level(LevelFilter::Off)
        .build();

    let mut loggers: Vec<Box<dyn SharedLogger>> = Vec::new();

    // File logger (always attempt)
    if let Ok(file) = fs::File::create(&log_path) {
        loggers.push(WriteLogger::new(LevelFilter::Debug, config.clone(), file));
    }

    // Terminal logger (for dev builds)
    loggers.push(TermLogger::new(
        LevelFilter::Info,
        config,
        TerminalMode::Mixed,
        ColorChoice::Auto,
    ));

    if CombinedLogger::init(loggers).is_err() {
        eprintln!("[LiteClip Replay] Warning: Failed to initialize logger");
    }

    info!("=== LiteClip Replay started ===");
    info!("Log file: {}", log_path.display());
}

/// Converts a [`HotkeyPreset`] into a concrete [`HotKey`] instance.
///
/// # Arguments
/// * `preset` - The preset configuration chosen by the user.
fn create_hotkey(preset: HotkeyPreset) -> HotKey {
    match preset {
        HotkeyPreset::F8 => HotKey::new(None, Code::F8),
        HotkeyPreset::F9 => HotKey::new(None, Code::F9),
        HotkeyPreset::F10 => HotKey::new(None, Code::F10),
        HotkeyPreset::CtrlShiftS => {
            HotKey::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyS)
        }
        HotkeyPreset::AltF9 => HotKey::new(Some(Modifiers::ALT), Code::F9),
    }
}

fn register_available_hotkey(
    manager: &GlobalHotKeyManager,
    preferred: HotkeyPreset,
) -> Option<(HotKey, HotkeyPreset)> {
    for preset in std::iter::once(preferred).chain(
        HotkeyPreset::all()
            .iter()
            .copied()
            .filter(|candidate| *candidate != preferred),
    ) {
        let hotkey = create_hotkey(preset);
        match manager.register(hotkey) {
            Ok(()) => return Some((hotkey, preset)),
            Err(e) => warn!("Hotkey {} unavailable: {}", preset.label(), e),
        }
    }
    None
}

/// Builds an icon as a white recording dot inside a larger black circle.
fn create_record_icon_rgba(size: u32) -> Vec<u8> {
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    let center = (size as f32 - 1.0) / 2.0;
    let outer_radius = size as f32 * 0.47;
    let inner_radius = size as f32 * 0.22;
    let outer_sq = outer_radius * outer_radius;
    let inner_sq = inner_radius * inner_radius;

    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let dist_sq = dx * dx + dy * dy;

            if dist_sq <= inner_sq {
                rgba.extend_from_slice(&[255, 255, 255, 255]);
            } else if dist_sq <= outer_sq {
                rgba.extend_from_slice(&[0, 0, 0, 255]);
            } else {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }

    rgba
}

fn create_window_icon_data() -> egui::IconData {
    let size = 64;
    egui::IconData {
        rgba: create_record_icon_rgba(size),
        width: size,
        height: size,
    }
}

/// Create a 32x32 RGBA tray icon.
fn create_tray_icon_image() -> tray_icon::Icon {
    let size = 32u32;
    tray_icon::Icon::from_rgba(create_record_icon_rgba(size), size, size)
        .expect("Failed to create tray icon")
}
