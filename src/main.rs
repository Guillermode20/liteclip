//! LiteClip Replay - CLI Entry Point with System Tray and GUI Support
//!
//! Application entry point with system tray integration and GUI settings.

use anyhow::Result;
use liteclip_replay::{
    app::AppState, config::Config, gui::run_settings_window_async, platform::PlatformHandle,
};
use std::env;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};
use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize compact logger: short, readable output with INFO as default.
    // Filter out noisy "Device::maintain" logs from wgpu while keeping other INFO logs.
    let filter = tracing_subscriber::filter::EnvFilter::new(
        "info,wgpu_core::device=warn,wgpu_hal::vulkan=warn",
    );

    tracing_subscriber::fmt()
        .compact()
        .with_target(false)
        .without_time()
        .with_level(true)
        .with_env_filter(filter)
        .init();

    let version = env!("CARGO_PKG_VERSION");
    println!("LiteClip Replay v{}", version);
    info!("LiteClip Replay {} starting", version);

    #[cfg(not(feature = "ffmpeg"))]
    warn!(
        "Built without FFmpeg support. Saved clips cannot be muxed to playable MP4. Rebuild with `cargo run --features ffmpeg`."
    );

    // Check for --gui flag
    let args: Vec<String> = env::args().collect();
    let gui_only = args.iter().any(|arg| arg == "--gui");

    // Load configuration from %APPDATA%/liteclip-replay/config.toml
    let mut config = match Config::load().await {
        Ok(cfg) => {
            let config_path = Config::config_path()?;
            info!("Loaded config: {:?}", config_path);
            cfg
        }
        Err(e) => {
            warn!("Failed to load config: {}. Using defaults.", e);
            Config::default()
        }
    };

    // Validate configuration — clamps invalid values to safe ranges
    config.validate();

    // If --gui flag is passed, just open the settings window
    if gui_only {
        info!("GUI mode: Opening settings window");
        let result = run_settings_window_async(config).await?;
        info!(
            "Settings window closed with result: saved={}, restart_required={}",
            result.saved, result.restart_required
        );
        return Ok(());
    }

    // Log configuration summary
    info!(
        "Config: {}s @ {} Mbps, {} FPS, codec={:?}, encoder={:?}, preset={:?}, rc={:?}, q={:?}",
        config.general.replay_duration_secs,
        config.video.bitrate_mbps,
        config.video.framerate,
        config.video.codec,
        config.video.encoder,
        config.video.quality_preset,
        config.video.rate_control,
        config.video.quality_value
    );

    // Initialize application state
    let app_state = Arc::new(RwLock::new(AppState::new(config.clone())?));

    // Start the platform message loop for hotkeys and tray
    let hotkey_config = hotkey_config_from_config(&config);
    let (platform_handle, event_rx) =
        liteclip_replay::platform::spawn_platform_thread(hotkey_config)?;
    let platform_handle = Arc::new(platform_handle);

    info!(
        "Hotkeys: save={} toggle={} (Ctrl+C exits)",
        config.hotkeys.save_clip, config.hotkeys.toggle_recording
    );

    // Initialize recording
    {
        let mut state = app_state.write().await;
        if let Err(e) = state.start_recording().await {
            error!("Failed to start recording: {}", e);
        }
    }

    // Convert the crossbeam receiver to a tokio-compatible channel
    let (tokio_tx, mut tokio_rx) =
        tokio::sync::mpsc::channel::<liteclip_replay::platform::AppEvent>(100);

    // Flag to track if GUI is open (to prevent multiple windows)
    let gui_open = Arc::new(RwLock::new(false));

    // Spawn a thread to bridge crossbeam events to tokio
    // Wrapped in catch_unwind to prevent silent panics from killing the event pipeline
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            while let Ok(event) = event_rx.recv() {
                if tokio_tx.blocking_send(event).is_err() {
                    break;
                }
            }
        }));
        if let Err(panic_info) = result {
            let msg = if let Some(s) = panic_info.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = panic_info.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic".to_string()
            };
            error!("Bridge thread panicked: {}", msg);
        }
    });

    // Main event loop
    loop {
        tokio::select! {
            // Handle platform events (hotkeys and tray)
            result = tokio::time::timeout(
                tokio::time::Duration::from_millis(100),
                tokio_rx.recv()
            ) => {
                match result {
                    Ok(Some(event)) => {
                        match event {
                            liteclip_replay::platform::AppEvent::Hotkey(action) => {
                                match action {
                                    liteclip_replay::platform::HotkeyAction::SaveClip => {
                                        info!("Hotkey: save clip");
                                        let state = app_state.read().await;
                                        match state.save_clip().await {
                                            Ok(path) => {
                                                info!("Clip saved: {:?}", path);
                                            }
                                            Err(e) => {
                                                error!("Failed to save clip: {:#}", e);
                                            }
                                        }
                                    }
                                    liteclip_replay::platform::HotkeyAction::ToggleRecording => {
                                        info!("Hotkey: toggle recording");
                                        let mut state = app_state.write().await;
                                        if state.is_recording() {
                                            if let Err(e) = state.stop_recording().await {
                                                error!("Failed to stop recording: {}", e);
                                            }
                                        } else if let Err(e) = state.start_recording().await {
                                            error!("Failed to start recording: {}", e);
                                        }
                                    }
                                    _ => {
                                        info!("Unhandled hotkey action: {:?}", action);
                                    }
                                }
                            }
                            liteclip_replay::platform::AppEvent::Tray(tray_event) => {
                                match tray_event {
                                    liteclip_replay::platform::TrayEvent::OpenSettings => {
                                        info!("Tray: Open Settings selected");
                                        handle_open_settings(
                                            gui_open.clone(),
                                            config.clone(),
                                            app_state.clone(),
                                            platform_handle.clone(),
                                        ).await;
                                    }
                                    liteclip_replay::platform::TrayEvent::SaveClip => {
                                        info!("Tray: Save Clip selected");
                                        let state = app_state.read().await;
                                        match state.save_clip().await {
                                            Ok(path) => {
                                                info!("Clip saved: {:?}", path);
                                            }
                                            Err(e) => {
                                                error!("Failed to save clip: {:#}", e);
                                            }
                                        }
                                    }
                                    liteclip_replay::platform::TrayEvent::ToggleRecording => {
                                        info!("Tray: Toggle Recording selected");
                                        let mut state = app_state.write().await;
                                        if state.is_recording() {
                                            if let Err(e) = state.stop_recording().await {
                                                error!("Failed to stop recording: {}", e);
                                            }
                                        } else if let Err(e) = state.start_recording().await {
                                            error!("Failed to start recording: {}", e);
                                        }
                                    }
                                    liteclip_replay::platform::TrayEvent::Exit => {
                                        info!("Tray: Exit selected");
                                        break;
                                    }
                                }
                            }
                            liteclip_replay::platform::AppEvent::Quit => {
                                info!("Quit signal received");
                                break;
                            }
                        }
                    }
                    Ok(None) => {
                        info!("Event channel closed");
                        break;
                    }
                    Err(_) => {
                        // Timeout - just loop again
                    }
                }
            }

            // Handle Ctrl+C
            _ = tokio::signal::ctrl_c() => {
                info!("Ctrl+C received");
                break;
            }
        }
    }

    // Cleanup
    info!("Shutting down");
    {
        let mut state = app_state.write().await;
        if let Err(e) = state.stop_recording().await {
            warn!("Error stopping recording: {}", e);
        }
    }

    platform_handle.join().ok();
    info!("LiteClip Replay stopped");

    Ok(())
}

/// Handle opening the settings window and processing the result
///
/// This function spawns the GUI, waits for it to close, and then applies
/// any configuration changes that don't require a restart.
async fn handle_open_settings(
    gui_open: Arc<RwLock<bool>>,
    current_config: Config,
    app_state: Arc<RwLock<AppState>>,
    platform_handle: Arc<PlatformHandle>,
) {
    // Check if GUI is already open
    let is_open = *gui_open.read().await;
    if is_open {
        warn!("Settings window already open");
        return;
    }

    // Clone values for the async task
    let gui_open_for_task = gui_open.clone();
    let config_for_gui = current_config.clone();
    let app_state_for_reload = app_state.clone();
    let platform_handle_for_reload = platform_handle.clone();

    tokio::spawn(async move {
        // Mark GUI as open
        *gui_open_for_task.write().await = true;

        // Run the settings window
        let result = run_settings_window_async(config_for_gui).await;

        // Mark GUI as closed
        *gui_open_for_task.write().await = false;

        match result {
            Ok(gui_result) => {
                info!(
                    "Settings window closed: saved={}, restart_required={}",
                    gui_result.saved, gui_result.restart_required
                );

                if gui_result.saved {
                    if let Some(new_config) = gui_result.new_config {
                        // Reload config from disk to ensure we have the latest
                        let reloaded_config = match Config::load().await {
                            Ok(cfg) => cfg,
                            Err(e) => {
                                warn!(
                                    "Failed to reload config from disk: {}. Using GUI result.",
                                    e
                                );
                                new_config
                            }
                        };

                        if gui_result.restart_required {
                            // Show restart notification
                            info!("Settings saved. Restart is required for some changes to take effect.");
                            show_restart_message_box();
                        } else {
                            // Apply runtime config changes
                            let mut state = app_state_for_reload.write().await;
                            if let Err(e) = state.apply_runtime_config(&reloaded_config) {
                                error!("Failed to apply runtime config: {}", e);
                            }
                            drop(state);

                            // Check if hotkey settings changed
                            let hotkeys_changed = current_config.hotkeys.save_clip
                                != reloaded_config.hotkeys.save_clip
                                || current_config.hotkeys.toggle_recording
                                    != reloaded_config.hotkeys.toggle_recording
                                || current_config.hotkeys.screenshot
                                    != reloaded_config.hotkeys.screenshot
                                || current_config.hotkeys.open_gallery
                                    != reloaded_config.hotkeys.open_gallery;

                            if hotkeys_changed {
                                info!("Hotkey settings changed, re-registering hotkeys");
                                let new_hotkey_config = hotkey_config_from_config(&reloaded_config);
                                if let Err(e) = platform_handle_for_reload
                                    .re_register_hotkeys(new_hotkey_config)
                                {
                                    error!("Failed to re-register hotkeys: {}", e);
                                } else {
                                    info!("Hotkeys re-registered successfully");
                                }
                            }

                            // Update the shared config reference
                            // Note: This won't update the caller's config, but new settings are applied
                            info!("Runtime configuration changes applied successfully");
                        }
                    }
                } else {
                    info!("Settings closed without saving");
                }
            }
            Err(e) => {
                error!("Settings window error: {}", e);
            }
        }
    });
}

/// Show a Windows message box informing the user that a restart is required
fn show_restart_message_box() {
    unsafe {
        // SAFETY: MessageBoxW is a standard Windows API call with null-terminated wide strings
        let title: Vec<u16> = "LiteClip Replay"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let message: Vec<u16> = "Some settings have been saved but require a restart to take effect.\n\nPlease restart LiteClip Replay to apply all changes."
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        // SAFETY: HWND can be null for MessageBoxW when no owner window is needed
        let hwnd = windows::Win32::Foundation::HWND(std::ptr::null_mut());
        MessageBoxW(
            hwnd,
            windows::core::PCWSTR(message.as_ptr()),
            windows::core::PCWSTR(title.as_ptr()),
            MB_ICONINFORMATION | MB_OK,
        );
    }
}

/// Convert Config to HotkeyConfig
fn hotkey_config_from_config(config: &Config) -> liteclip_replay::platform::HotkeyConfig {
    liteclip_replay::platform::HotkeyConfig {
        save_clip: config.hotkeys.save_clip.clone(),
        toggle_recording: config.hotkeys.toggle_recording.clone(),
        screenshot: config.hotkeys.screenshot.clone(),
        open_gallery: config.hotkeys.open_gallery.clone(),
    }
}
