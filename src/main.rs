//! LiteClip Replay - System Tray Only Entry Point
//!
//! Application entry point with system tray integration.
//! Runs as a background application with no GUI window.

// Hide console window on Windows in release, but keep console in debug so logs are visible.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use anyhow::Result;
use liteclip_replay::{app::AppState, config::Config};
use std::env;
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};
#[cfg(windows)]
use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod, TIMERR_NOERROR};

const CREATE_NO_WINDOW: u32 = 0x08000000;

#[cfg(windows)]
struct TimerResolutionGuard {
    active: bool,
    period_ms: u32,
}

#[cfg(windows)]
impl TimerResolutionGuard {
    fn new(period_ms: u32) -> Self {
        let result = unsafe { timeBeginPeriod(period_ms) };
        let active = result == TIMERR_NOERROR;
        if active {
            info!("Enabled {}ms timer resolution", period_ms);
        } else {
            warn!(
                "Failed to enable {}ms timer resolution (MMRESULT={})",
                period_ms, result
            );
        }
        Self { active, period_ms }
    }
}

#[cfg(windows)]
impl Drop for TimerResolutionGuard {
    fn drop(&mut self) {
        if self.active {
            let result = unsafe { timeEndPeriod(self.period_ms) };
            if result != TIMERR_NOERROR {
                warn!("Failed to restore timer resolution (MMRESULT={})", result);
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize compact logger: short, readable output with INFO as default.
    let filter = tracing_subscriber::filter::EnvFilter::new("info");

    tracing_subscriber::fmt()
        .compact()
        .with_target(false)
        .without_time()
        .with_level(true)
        .with_env_filter(filter)
        .init();

    #[cfg(windows)]
    let _timer_resolution_guard = TimerResolutionGuard::new(1);

    let version = env!("CARGO_PKG_VERSION");
    println!("LiteClip Replay v{}", version);
    info!("LiteClip Replay {} starting", version);

    // Load configuration from %APPDATA%/liteclip-replay/config.toml
    let config = match Config::load().await {
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
    let mut config = config;
    config.validate();

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

    // Show welcome notification if not starting minimized
    if !config.general.start_minimised {
        info!("LiteClip Replay is running in the system tray");
        info!("Right-click the tray icon to access settings");
    } else {
        info!("Started minimized to system tray");
    }

    // Initialize application state
    let app_state = Arc::new(RwLock::new(AppState::new(config.clone())?));

    // Start the platform message loop for hotkeys and tray
    let hotkey_config = hotkey_config_from_config(&config);
    info!("Spawning platform thread...");
    let (platform_handle, event_rx) =
        liteclip_replay::platform::spawn_platform_thread(hotkey_config)?;
    info!("Platform thread spawned, handle created");
    let platform_handle = Arc::new(platform_handle);

    info!(
        "Hotkeys: save={} toggle={} (Ctrl+C exits)",
        config.hotkeys.save_clip, config.hotkeys.toggle_recording
    );

    // Convert the crossbeam receiver to a tokio-compatible channel
    let (tokio_tx, mut tokio_rx) =
        tokio::sync::mpsc::channel::<liteclip_replay::platform::AppEvent>(100);

    // Bridge: hotkey/tray crossbeam events -> tokio
    let tokio_tx_bridge = tokio_tx.clone();
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            while let Ok(event) = event_rx.recv() {
                if tokio_tx_bridge.blocking_send(event).is_err() {
                    break;
                }
            }
        }));
        if let Err(e) = result {
            error!("Event bridge panicked: {:?}", e);
        }
    });

    // Initialize recording
    {
        let mut state = app_state.write().await;
        if let Err(e) = state.start_recording().await {
            error!("Failed to start recording: {}", e);
        } else {
            let _ = platform_handle.update_recording_state(true);
        }
    }

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
                                                let _ = platform_handle.show_notification(
                                                    "Clip Saved",
                                                    &format!("Saved to {:?}", path.file_name().unwrap_or_default()),
                                                );
                                            }
                                            Err(e) => {
                                                error!("Failed to save clip: {:#}", e);
                                                let _ = platform_handle.show_notification(
                                                    "Save Failed",
                                                    &format!("{:#}", e),
                                                );
                                            }
                                        }
                                    }
                                    liteclip_replay::platform::HotkeyAction::ToggleRecording => {
                                        info!("Hotkey: toggle recording");
                                        let mut state = app_state.write().await;
                                        if state.is_recording() {
                                            if let Err(e) = state.stop_recording().await {
                                                error!("Failed to stop recording: {}", e);
                                                let _ = platform_handle.show_notification(
                                                    "Recording Error",
                                                    &format!("Failed to stop: {}", e),
                                                );
                                            } else {
                                                let _ = platform_handle.update_recording_state(false);
                                                let _ = platform_handle.show_notification(
                                                    "Recording Stopped",
                                                    "Recording has been stopped",
                                                );
                                            }
                                        } else if let Err(e) = state.start_recording().await {
                                            error!("Failed to start recording: {}", e);
                                            let _ = platform_handle.show_notification(
                                                "Recording Error",
                                                &format!("Failed to start: {}", e),
                                            );
                                        } else {
                                            let _ = platform_handle.update_recording_state(true);
                                            let _ = platform_handle.show_notification(
                                                "Recording Started",
                                                "Now capturing replay buffer",
                                            );
                                        }
                                    }
                                    liteclip_replay::platform::HotkeyAction::Screenshot => {
                                        info!("Hotkey: screenshot (not implemented)");
                                        warn!("Screenshot feature not yet implemented");
                                    }
                                    liteclip_replay::platform::HotkeyAction::OpenGallery => {
                                        info!("Hotkey: open gallery (not implemented)");
                                        warn!("Gallery feature not yet implemented");
                                    }
                                }
                            }
                            liteclip_replay::platform::AppEvent::Tray(tray_event) => {
                                match tray_event {
                                    liteclip_replay::platform::TrayEvent::SaveClip => {
                                        info!("Tray: Save Clip selected");
                                        let state = app_state.read().await;
                                        match state.save_clip().await {
                                            Ok(path) => {
                                                info!("Clip saved: {:?}", path);
                                                let _ = platform_handle.show_notification(
                                                    "Clip Saved",
                                                    &format!("Saved to {:?}", path.file_name().unwrap_or_default()),
                                                );
                                            }
                                            Err(e) => {
                                                error!("Failed to save clip: {:#}", e);
                                                let _ = platform_handle.show_notification(
                                                    "Save Failed",
                                                    &format!("{:#}", e),
                                                );
                                            }
                                        }
                                    }
                                    liteclip_replay::platform::TrayEvent::Exit => {
                                        info!("Tray: Exit selected");
                                        break;
                                    }
                                    liteclip_replay::platform::TrayEvent::OpenSettings => {
                                        info!("Tray: Open Settings selected");
                                        match Config::config_path() {
                                            Ok(path) => {
                                                let result = Command::new("cmd")
                                                    .args(["/C", "start", "", &path.to_string_lossy()])
                                                    .creation_flags(CREATE_NO_WINDOW)
                                                    .spawn();
                                                if let Err(e) = result {
                                                    error!("Failed to open settings file: {}", e);
                                                    let _ = platform_handle.show_notification(
                                                        "Error",
                                                        &format!("Failed to open settings: {}", e),
                                                    );
                                                }
                                            }
                                            Err(e) => {
                                                error!("Failed to get config path: {}", e);
                                            }
                                        }
                                    }
                                    _ => {
                                        // Other tray events are not used (StartRecording, StopRecording,
                                        // ToggleRecording)
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

    // Signal platform thread to exit its message loop, then wait for it
    if let Err(e) = platform_handle.quit() {
        warn!("Failed to send quit to platform thread: {}", e);
    }
    platform_handle.join().ok();
    info!("LiteClip Replay stopped");

    Ok(())
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
