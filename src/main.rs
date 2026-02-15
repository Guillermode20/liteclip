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
    GlobalHotKeyEvent, GlobalHotKeyManager,
};
use log::{error, info};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
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

    // --- Register initial global hotkey ---
    let hotkey_manager = GlobalHotKeyManager::new().expect("Failed to init hotkey manager");
    let initial_hotkey = {
        let rec = recorder.lock().unwrap();
        rec.settings.hotkey
    };
    let current_hotkey = create_hotkey(initial_hotkey);
    hotkey_manager
        .register(current_hotkey)
        .expect("Failed to register hotkey");
    let current_hotkey_id = current_hotkey.id();

    info!("Initial hotkey registered: {}", initial_hotkey.label());

    // --- System tray icon ---
    let tray_menu = Menu::new();
    let show_item = MenuItem::new("Show LiteClip", true, None);
    let quit_item = MenuItem::new("Quit", true, None);
    let _ = tray_menu.append(&show_item);
    let _ = tray_menu.append(&quit_item);

    let show_item_id = show_item.id().clone();
    let quit_item_id = quit_item.id().clone();

    // Create a simple 32x32 RGBA icon (a filled white/blue square)
    let icon = create_tray_icon_image();

    let _tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(tray_menu))
        .with_tooltip("LiteClip — Replay Buffer")
        .with_icon(icon)
        .build()
        .expect("Failed to create tray icon");

    // --- Shared flags for tray menu actions ---
    // A background thread polls MenuEvent (which works even when the window is hidden)
    // and sets these flags. The update() loop reads them.
    let tray_show_requested = Arc::new(AtomicBool::new(false));
    let tray_quit_requested = Arc::new(AtomicBool::new(false));

    let show_flag = tray_show_requested.clone();
    let recorder_for_quit = recorder.clone();

    // Spawn a dedicated thread to listen for tray menu events
    std::thread::spawn(move || {
        let menu_rx = MenuEvent::receiver();
        loop {
            // Blocking receive — wakes only on menu clicks
            if let Ok(event) = menu_rx.recv() {
                if event.id == show_item_id {
                    info!("Tray thread: Show requested");
                    show_flag.store(true, Ordering::SeqCst);
                } else if event.id == quit_item_id {
                    info!("Tray thread: Quit — stopping recorder and exiting");
                    // Stop recorder directly from this thread, then exit
                    if let Ok(mut rec) = recorder_for_quit.lock() {
                        rec.stop();
                    }
                    std::process::exit(0);
                }
            }
        }
    });

    let recorder_for_app = recorder.clone();

    // --- Window options ---
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("LiteClip")
            .with_inner_size([340.0, 320.0])
            .with_min_inner_size([300.0, 260.0])
            .with_max_inner_size([500.0, 500.0])
            .with_always_on_top(),
        ..Default::default()
    };

    eframe::run_native(
        "LiteClip",
        options,
        Box::new(move |_cc| {
            Ok(Box::new(HotkeyWrapper {
                app: LiteClipApp::new(recorder_for_app),
                hotkey_manager,
                current_hotkey,
                current_hotkey_id,
                tray_show_requested,
                tray_quit_requested,
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
    hotkey_manager: GlobalHotKeyManager,
    /// The currently registered hotkey configuration
    current_hotkey: HotKey,
    /// The unique ID of the currently registered hotkey
    current_hotkey_id: u32,
    /// Set by background thread when "Show LiteClip" is clicked in tray
    tray_show_requested: Arc<AtomicBool>,
    /// Set by background thread when "Quit" is clicked in tray
    tray_quit_requested: Arc<AtomicBool>,
}

impl eframe::App for HotkeyWrapper {
    /// Updates the application state every frame.
    /// Polls for hotkey events, tray flags, and delegates UI rendering to [`LiteClipApp`].
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // --- Handle tray quit (checked first, works even after window hidden) ---
        if self.tray_quit_requested.swap(false, Ordering::SeqCst) {
            info!("Tray: Quit — stopping recorder and exiting");
            if let Ok(mut rec) = self.app.recorder.try_lock() {
                rec.stop();
            }
            std::process::exit(0);
        }

        // --- Handle tray show ---
        if self.tray_show_requested.swap(false, Ordering::SeqCst) {
            info!("Tray: Showing window");
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
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
                // Cancel the close and hide the window instead
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                info!("Window hidden to tray");
            }
        }

        // Check for hotkey events
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            if event.id == self.current_hotkey_id {
                info!("Global hotkey triggered — saving clip");
                self.app.trigger_save();
            }
        }

        // Check if the GUI requested a hotkey change
        if let Some(new_preset) = self.app.pending_hotkey.take() {
            // Unregister old hotkey
            let _ = self.hotkey_manager.unregister(self.current_hotkey);

            // Register new hotkey
            let new_hotkey = create_hotkey(new_preset);
            match self.hotkey_manager.register(new_hotkey) {
                Ok(()) => {
                    info!("Hotkey changed to {}", new_preset.label());
                    self.current_hotkey = new_hotkey;
                    self.current_hotkey_id = new_hotkey.id();
                    self.app.status_message = format!("Hotkey changed to {}", new_preset.label());
                    self.app.status_timer = 3.0;
                }
                Err(e) => {
                    error!("Failed to register hotkey {}: {}", new_preset.label(), e);
                    // Re-register old hotkey on failure
                    let _ = self.hotkey_manager.register(self.current_hotkey);
                    self.app.status_message = format!("Hotkey error: {}", e);
                    self.app.status_timer = 5.0;
                }
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
        eprintln!("[LiteClip] Warning: Failed to initialize logger");
    }

    info!("=== LiteClip started ===");
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

/// Create a simple 32×32 RGBA tray icon (a filled colored square).
fn create_tray_icon_image() -> tray_icon::Icon {
    let size = 32u32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            // Draw a rounded-ish icon: white center with dark border
            let border = x == 0 || y == 0 || x == size - 1 || y == size - 1;
            if border {
                rgba.extend_from_slice(&[40, 40, 40, 255]); // dark border
            } else {
                rgba.extend_from_slice(&[220, 230, 255, 255]); // light blue-white fill
            }
        }
    }
    tray_icon::Icon::from_rgba(rgba, size, size).expect("Failed to create tray icon")
}
