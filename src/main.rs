//! LiteClip Recorder
//!
//! A lightweight screen recording application with a rolling replay buffer,
//! similar to Medal.tv or ShadowPlay. Built with Rust, eframe (egui), and Windows native capture APIs.
//!
//! Architecture:
//! - Main thread runs the core service (recorder, hotkeys, tray icon)
//! - GUI runs in a separate thread and stays alive for the app lifetime
//! - Communication uses shared flags plus GUI command channels

mod gui;
mod recorder;
mod settings;

use eframe::egui;
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use log::{error, info, warn};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem},
    TrayIconBuilder,
};

#[cfg(windows)]
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use gui::LiteClipApp;
use recorder::Recorder;
use settings::HotkeyPreset;

/// Message from GUI thread to main thread requesting a hotkey change
#[derive(Debug, Clone, Copy)]
pub enum HotkeyChangeRequest {
    Change(HotkeyPreset),
}

/// Signal from main/tray threads to GUI thread for window lifecycle control.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GuiWindowCommand {
    Show,
    Exit,
}

/// Shared state between GUI thread and main thread
pub struct GuiState {
    /// Set by tray when Quit is clicked, or by GUI when ExitApp requested
    pub should_exit: AtomicBool,
    /// Channel sender for hotkey change requests (GUI -> main)
    pub hotkey_tx: Mutex<Option<Sender<HotkeyChangeRequest>>>,
    /// Channel sender for GUI window commands (main/tray -> GUI)
    pub visibility_tx: Mutex<Option<Sender<GuiWindowCommand>>>,
    /// Stored egui context to wake the event loop when window is hidden
    pub egui_ctx: Mutex<Option<egui::Context>>,
    /// Native HWND for direct show/hide operations from non-GUI threads.
    pub window_hwnd: AtomicIsize,
}

impl GuiState {
    pub fn new() -> Self {
        Self {
            should_exit: AtomicBool::new(false),
            hotkey_tx: Mutex::new(None),
            visibility_tx: Mutex::new(None),
            egui_ctx: Mutex::new(None),
            window_hwnd: AtomicIsize::new(0),
        }
    }
}

impl Default for GuiState {
    fn default() -> Self {
        Self::new()
    }
}

/// Main entry point for the LiteClip application.
/// Initializes the background service (recorder, hotkeys, tray) and manages
/// the GUI lifecycle in a separate thread.
fn main() {
    init_logging();
    info!("=== LiteClip Replay starting ===");

    // --- Shared state ---
    let recorder = Arc::new(Mutex::new(Recorder::new()));
    let gui_state = Arc::new(GuiState::new());

    // --- Ctrl+C handler ---
    {
        let recorder_for_ctrlc = recorder.clone();
        let gui_state_for_ctrlc = gui_state.clone();
        let _ = ctrlc::set_handler(move || {
            info!("Ctrl+C received — stopping recorder");
            if let Ok(mut rec) = recorder_for_ctrlc.lock() {
                rec.stop();
            }
            gui_state_for_ctrlc
                .should_exit
                .store(true, Ordering::SeqCst);
            std::process::exit(0);
        });
    }

    // --- Register global hotkey ---
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

    // --- Spawn tray menu listener thread ---
    let gui_state_for_tray = gui_state.clone();
    std::thread::spawn(move || {
        let menu_rx = MenuEvent::receiver();
        loop {
            if let Ok(event) = menu_rx.recv() {
                if event.id == show_item_id {
                    info!("Tray: Show requested");
                    let hwnd = gui_state_for_tray.window_hwnd.load(Ordering::SeqCst);
                    if hwnd != 0 {
                        restore_window_native(hwnd);
                    }
                    if let Ok(guard) = gui_state_for_tray.egui_ctx.lock() {
                        if let Some(ref ctx) = *guard {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                            ctx.request_repaint();
                        }
                    }
                    // Keep command-channel path as a fallback/state sync when the GUI loop is active.
                    if let Ok(guard) = gui_state_for_tray.visibility_tx.lock() {
                        if let Some(ref tx) = *guard {
                            let _ = tx.send(GuiWindowCommand::Show);
                        }
                    }
                } else if event.id == quit_item_id {
                    info!("Tray: Quit requested");
                    gui_state_for_tray.should_exit.store(true, Ordering::SeqCst);
                    let hwnd = gui_state_for_tray.window_hwnd.load(Ordering::SeqCst);
                    if hwnd != 0 {
                        close_window_native(hwnd);
                    }
                    if let Ok(guard) = gui_state_for_tray.egui_ctx.lock() {
                        if let Some(ref ctx) = *guard {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            ctx.request_repaint();
                        }
                    }
                    if let Ok(guard) = gui_state_for_tray.visibility_tx.lock() {
                        if let Some(ref tx) = *guard {
                            let _ = tx.send(GuiWindowCommand::Exit);
                        }
                    }
                }
            }
        }
    });

    // --- Spawn global hotkey listener thread ---
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

    // --- Spawn recorder watchdog thread ---
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

    // --- Main service loop ---
    // This loop manages the GUI visibility and handles hotkey changes
    info!("Entering main service loop");

    // Move resources needed by GUI into Arcs for sharing
    let recorder_for_gui = recorder.clone();
    let hotkey_id_shared_for_gui = hotkey_id_shared.clone();

    // Mutable state for hotkey management (stays in main thread)
    let mut current_hotkey_mut = current_hotkey;
    let mut current_hotkey_id_mut = current_hotkey_id;

    // Create channels for GUI communication
    let (hotkey_tx, hotkey_rx) = channel::<HotkeyChangeRequest>();
    if let Ok(mut guard) = gui_state.hotkey_tx.lock() {
        *guard = Some(hotkey_tx);
    }

    let (vis_tx, vis_rx) = channel::<GuiWindowCommand>();
    if let Ok(mut guard) = gui_state.visibility_tx.lock() {
        *guard = Some(vis_tx.clone());
    }

    // Spawn GUI thread once - it stays alive and listens for visibility commands
    info!("Spawning GUI thread");
    let gs = gui_state.clone();
    let rec = recorder_for_gui.clone();
    let gui_thread = std::thread::spawn(move || {
        run_gui_thread(rec, gs, vis_rx);
    });

    loop {
        // Check if we should exit
        if gui_state.should_exit.load(Ordering::SeqCst) {
            info!("Exit signal received, shutting down...");
            // Signal GUI to exit and wake its event loop.
            if let Ok(guard) = gui_state.visibility_tx.lock() {
                if let Some(ref tx) = *guard {
                    let _ = tx.send(GuiWindowCommand::Exit);
                }
            }
            if let Ok(guard) = gui_state.egui_ctx.lock() {
                if let Some(ref ctx) = *guard {
                    ctx.request_repaint();
                }
            }
            // Wait for GUI thread to finish
            info!("Waiting for GUI thread to finish...");
            let _ = gui_thread.join();
            // Stop recorder
            if let Ok(mut rec) = recorder.lock() {
                rec.stop();
            }
            break;
        }

        // Check if GUI thread finished unexpectedly
        if gui_thread.is_finished() {
            error!("GUI thread finished unexpectedly");
            gui_state.should_exit.store(true, Ordering::SeqCst);
            continue;
        }

        // Process any pending hotkey change requests
        while let Ok(request) = hotkey_rx.try_recv() {
            match request {
                HotkeyChangeRequest::Change(new_preset) => {
                    info!(
                        "Main: Processing hotkey change request to {}",
                        new_preset.label()
                    );
                    if let Some(ref manager) = hotkey_manager {
                        let new_hotkey = create_hotkey(new_preset);
                        if new_hotkey.id() != current_hotkey_id_mut {
                            match manager.register(new_hotkey) {
                                Ok(()) => {
                                    if let Some(old_hotkey) = current_hotkey_mut.take() {
                                        let _ = manager.unregister(old_hotkey);
                                    }
                                    info!("Hotkey changed to {}", new_preset.label());
                                    current_hotkey_mut = Some(new_hotkey);
                                    current_hotkey_id_mut = new_hotkey.id();
                                    hotkey_id_shared_for_gui
                                        .store(current_hotkey_id_mut, Ordering::SeqCst);
                                    if let Ok(mut rec) = recorder_for_gui.lock() {
                                        rec.settings.hotkey = new_preset;
                                        rec.settings.save();
                                    }
                                }
                                Err(e) => {
                                    error!(
                                        "Failed to register hotkey {}: {}",
                                        new_preset.label(),
                                        e
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        // Pump Windows messages for the tray icon to work properly
        #[cfg(windows)]
        {
            use windows::Win32::UI::WindowsAndMessaging::{PeekMessageW, MSG, PM_REMOVE};
            unsafe {
                let mut msg: MSG = std::mem::zeroed();
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).into() {
                    let _ = windows::Win32::UI::WindowsAndMessaging::TranslateMessage(&msg);
                    let _ = windows::Win32::UI::WindowsAndMessaging::DispatchMessageW(&msg);
                }
            }
        }

        // Small sleep to avoid busy-waiting
        std::thread::sleep(Duration::from_millis(50));
    }

    info!("=== LiteClip Replay stopped ===");
}

/// Runs the GUI in a separate thread.
/// This function returns when the GUI window is closed.
fn run_gui_thread(
    recorder: Arc<Mutex<Recorder>>,
    gui_state: Arc<GuiState>,
    visibility_rx: std::sync::mpsc::Receiver<GuiWindowCommand>,
) {
    info!("GUI thread started");

    #[cfg(windows)]
    let event_loop_builder = Some(Box::new(
        |builder: &mut winit::event_loop::EventLoopBuilder<eframe::UserEvent>| {
            // On Windows, we need to allow the event loop on non-main threads
            #[cfg(windows)]
            {
                use winit::platform::windows::EventLoopBuilderExtWindows;
                builder.with_any_thread(true);
            }
        },
    )
        as Box<dyn FnOnce(&mut winit::event_loop::EventLoopBuilder<eframe::UserEvent>)>);

    #[cfg(not(windows))]
    let event_loop_builder: Option<
        Box<dyn FnOnce(&mut winit::event_loop::EventLoopBuilder<eframe::UserEvent>)>,
    > = None;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("LiteClip Replay")
            .with_inner_size([340.0, 320.0])
            .with_min_inner_size([300.0, 260.0])
            .with_max_inner_size([500.0, 500.0])
            .with_always_on_top()
            .with_visible(true)
            .with_icon(create_window_icon_data()),
        run_and_return: true,
        event_loop_builder,
        ..Default::default()
    };

    let result = eframe::run_native(
        "LiteClip Replay",
        options,
        Box::new(move |_cc| {
            Ok(Box::new(HotkeyWrapper {
                app: LiteClipApp::new(recorder),
                gui_state,
                visibility_rx,
                is_hidden: false,
                last_minimized_state: false,
            }))
        }),
    );

    if let Err(e) = result {
        error!("GUI error: {:?}", e);
    }

    info!("GUI thread exiting");
}

/// Wraps the LiteClipApp to handle close actions.
struct HotkeyWrapper {
    /// The actual LiteClip GUI application
    app: LiteClipApp,
    /// Shared state with main thread
    gui_state: Arc<GuiState>,
    /// Channel receiver for window commands from main/tray
    visibility_rx: std::sync::mpsc::Receiver<GuiWindowCommand>,
    /// Tracks if window is currently hidden (minimized to tray)
    is_hidden: bool,
    /// Last observed native minimized state to detect transitions.
    last_minimized_state: bool,
}

impl HotkeyWrapper {
    #[cfg(windows)]
    fn cache_window_handle(&self, frame: &eframe::Frame) {
        if let Ok(handle) = frame.window_handle() {
            if let RawWindowHandle::Win32(window_handle) = handle.as_raw() {
                let hwnd = window_handle.hwnd.get();
                let previous = self.gui_state.window_hwnd.swap(hwnd, Ordering::SeqCst);
                if previous != hwnd {
                    info!("GUI: Captured native window handle");
                }
            }
        }
    }

    #[cfg(not(windows))]
    fn cache_window_handle(&self, _frame: &eframe::Frame) {}

    fn minimize_to_tray_enabled(&self) -> bool {
        self.app
            .recorder
            .try_lock()
            .map(|rec| rec.settings.minimize_to_tray)
            .unwrap_or(false)
    }

    fn minimize_to_tray(&mut self, ctx: &egui::Context, reason: &str) {
        if self.is_hidden {
            return;
        }

        info!("GUI: Minimizing to tray ({})", reason);
        self.is_hidden = true;
        self.last_minimized_state = false;
        let hwnd = self.gui_state.window_hwnd.load(Ordering::SeqCst);
        if hwnd != 0 {
            hide_window_native(hwnd);
        } else {
            // Fallback if native handle isn't available yet.
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
    }

    fn maximize_from_tray(&mut self, ctx: &egui::Context) {
        info!("GUI: Restoring window from tray");
        self.is_hidden = false;
        self.last_minimized_state = false;
        let hwnd = self.gui_state.window_hwnd.load(Ordering::SeqCst);
        if hwnd != 0 {
            restore_window_native(hwnd);
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
    }

    fn request_window_exit(&self, ctx: &egui::Context) {
        let hwnd = self.gui_state.window_hwnd.load(Ordering::SeqCst);
        if hwnd != 0 {
            close_window_native(hwnd);
        } else {
            // Make sure the root viewport can close even if currently hidden/minimized.
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

impl eframe::App for HotkeyWrapper {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.cache_window_handle(frame);

        if let Ok(mut guard) = self.gui_state.egui_ctx.lock() {
            *guard = Some(ctx.clone());
        }

        // Apply pending window commands from tray/main thread.
        while let Ok(cmd) = self.visibility_rx.try_recv() {
            match cmd {
                GuiWindowCommand::Show => self.maximize_from_tray(ctx),
                GuiWindowCommand::Exit => {
                    info!("GUI: Exit command received");
                    self.request_window_exit(ctx);
                    return;
                }
            }
        }

        if self.gui_state.should_exit.load(Ordering::SeqCst) {
            self.request_window_exit(ctx);
            return;
        }

        let minimize_to_tray = self.minimize_to_tray_enabled();
        let window_minimized = ctx.input(|i| i.viewport().minimized.unwrap_or(false));

        // Treat an OS-level minimize action as "minimize to tray" when enabled.
        if minimize_to_tray && !self.is_hidden && window_minimized && !self.last_minimized_state {
            self.minimize_to_tray(ctx, "native minimize");
        }

        if ctx.input(|i| i.viewport().close_requested()) {
            if minimize_to_tray {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.minimize_to_tray(ctx, "window close");
            } else {
                info!("GUI: Exit requested by close button");
                self.gui_state.should_exit.store(true, Ordering::SeqCst);
                self.request_window_exit(ctx);
                return;
            }
        }

        self.last_minimized_state = if self.is_hidden {
            false
        } else {
            window_minimized
        };

        // Only update the app UI if not hidden
        if !self.is_hidden {
            // Check if the GUI requested a hotkey change
            if let Some(new_preset) = self.app.pending_hotkey.take() {
                // Send the hotkey change request to the main thread via channel
                if let Ok(tx_guard) = self.gui_state.hotkey_tx.lock() {
                    if let Some(ref tx) = *tx_guard {
                        if let Err(e) = tx.send(HotkeyChangeRequest::Change(new_preset)) {
                            error!("Failed to send hotkey change request: {}", e);
                            self.app.status_message = "Failed to change hotkey.".into();
                            self.app.status_timer = 5.0;
                        } else {
                            // The change will be processed by the main thread
                            // We'll update the display optimistically
                            self.app.status_message =
                                format!("Hotkey change requested: {}", new_preset.label());
                            self.app.status_timer = 3.0;
                        }
                    } else {
                        self.app.status_message =
                            "Global hotkeys are unavailable on this system.".into();
                        self.app.status_timer = 5.0;
                    }
                }
            }

            // Delegate to the actual app
            self.app.update(ctx, frame);
        }

        // Keep the event loop alive
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
}

/// Initializes the logging system using `simplelog`.
fn init_logging() {
    use simplelog::*;
    use std::fs;

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

    if let Ok(file) = fs::File::create(&log_path) {
        loggers.push(WriteLogger::new(LevelFilter::Debug, config.clone(), file));
    }

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

#[cfg(windows)]
fn hide_window_native(hwnd: isize) {
    use std::ffi::c_void;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{ShowWindowAsync, SW_HIDE};

    unsafe {
        let _ = ShowWindowAsync(HWND(hwnd as *mut c_void), SW_HIDE);
    }
}

#[cfg(not(windows))]
fn hide_window_native(_hwnd: isize) {}

#[cfg(windows)]
fn restore_window_native(hwnd: isize) {
    use std::ffi::c_void;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        SetForegroundWindow, ShowWindowAsync, SW_RESTORE, SW_SHOW,
    };

    unsafe {
        let target = HWND(hwnd as *mut c_void);
        let _ = ShowWindowAsync(target, SW_RESTORE);
        let _ = ShowWindowAsync(target, SW_SHOW);
        let _ = SetForegroundWindow(target);
    }
}

#[cfg(not(windows))]
fn restore_window_native(_hwnd: isize) {}

#[cfg(windows)]
fn close_window_native(hwnd: isize) {
    use std::ffi::c_void;
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};

    unsafe {
        let _ = PostMessageW(HWND(hwnd as *mut c_void), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
}

#[cfg(not(windows))]
fn close_window_native(_hwnd: isize) {}

/// Create a 32x32 RGBA tray icon.
fn create_tray_icon_image() -> tray_icon::Icon {
    let size = 32u32;
    tray_icon::Icon::from_rgba(create_record_icon_rgba(size), size, size)
        .expect("Failed to create tray icon")
}
