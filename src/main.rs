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
use std::sync::atomic::{AtomicBool, Ordering};
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

    // Create a job object and assign the current process to it.
    // This ensures any child processes like ffmpeg are grouped under
    // the main app in Windows Task Manager and automatically killed
    // when the main application exits.
    #[cfg(windows)]
    {
        use windows::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            SetInformationJobObject,
        };
        use windows::Win32::System::Threading::GetCurrentProcess;
        use windows::core::PCWSTR;

        unsafe {
            if let Ok(job) = CreateJobObjectW(None, PCWSTR::null()) {
                let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let _ = SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                let _ = AssignProcessToJobObject(job, GetCurrentProcess());
                // We purposefully leak the job handle so it lives as long as the process.
                std::mem::forget(job);
            } else {
                warn!("Failed to create Job Object. Child processes won't be grouped in Task Manager.");
            }
        }
    }

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

    if config.general.auto_start_with_windows {
        warn!(
            "Config: auto_start_with_windows=true, but startup registration is not implemented yet"
        );
    }
    if config.general.auto_detect_game {
        warn!("Config: auto_detect_game=true, but game detection is not implemented yet");
    }
    if config.advanced.overlay_enabled {
        warn!("Config: overlay_enabled=true, but overlay rendering is not implemented yet");
    }

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

    let mut should_restart = false;
    let save_in_progress = Arc::new(AtomicBool::new(false));

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
                                        if save_in_progress
                                            .compare_exchange(
                                                false,
                                                true,
                                                Ordering::SeqCst,
                                                Ordering::SeqCst,
                                            )
                                            .is_err()
                                        {
                                            info!("Save request ignored: clip save already in progress");
                                            continue;
                                        }
                                        let (config, buffer, notifications_enabled) =
                                            app_state.read().await.save_context();
                                        let platform_handle_clone = platform_handle.clone();
                                        let save_in_progress_clone = save_in_progress.clone();
                                        tokio::spawn(async move {
                                            let result = liteclip_replay::app::ClipManager::save_clip(
                                                &config, &buffer,
                                            )
                                            .await;
                                            match result {
                                                Ok(path) => {
                                                    info!("Clip saved: {:?}", path);
                                                    if notifications_enabled {
                                                        let _ = platform_handle_clone.show_notification(
                                                            "Clip Saved",
                                                            &format!(
                                                                "Saved to {:?}",
                                                                path.file_name().unwrap_or_default()
                                                            ),
                                                        );
                                                    }
                                                }
                                                Err(e) => {
                                                    error!("Failed to save clip: {:#}", e);
                                                    if notifications_enabled {
                                                        let _ = platform_handle_clone.show_notification(
                                                            "Save Failed",
                                                            &format!("{:#}", e),
                                                        );
                                                    }
                                                }
                                            }
                                            save_in_progress_clone.store(false, Ordering::SeqCst);
                                        });
                                    }
                                    liteclip_replay::platform::HotkeyAction::ToggleRecording => {
                                        info!("Hotkey: toggle recording");
                                        let mut state = app_state.write().await;
                                        let notifications_enabled = state.config().general.notifications;
                                        if state.is_recording() {
                                            if let Err(e) = state.stop_recording().await {
                                                error!("Failed to stop recording: {}", e);
                                                if notifications_enabled {
                                                    let _ = platform_handle.show_notification(
                                                        "Recording Error",
                                                        &format!("Failed to stop: {}", e),
                                                    );
                                                }
                                            } else {
                                                let _ = platform_handle.update_recording_state(false);
                                                if notifications_enabled {
                                                    let _ = platform_handle.show_notification(
                                                        "Recording Stopped",
                                                        "Recording has been stopped",
                                                    );
                                                }
                                            }
                                        } else if let Err(e) = state.start_recording().await {
                                            error!("Failed to start recording: {}", e);
                                            if notifications_enabled {
                                                let _ = platform_handle.show_notification(
                                                    "Recording Error",
                                                    &format!("Failed to start: {}", e),
                                                );
                                            }
                                        } else {
                                            let _ = platform_handle.update_recording_state(true);
                                            if notifications_enabled {
                                                let _ = platform_handle.show_notification(
                                                    "Recording Started",
                                                    "Now capturing replay buffer",
                                                );
                                            }
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
                                        if save_in_progress
                                            .compare_exchange(
                                                false,
                                                true,
                                                Ordering::SeqCst,
                                                Ordering::SeqCst,
                                            )
                                            .is_err()
                                        {
                                            info!("Save request ignored: clip save already in progress");
                                            continue;
                                        }
                                        let (config, buffer, notifications_enabled) =
                                            app_state.read().await.save_context();
                                        let platform_handle_clone = platform_handle.clone();
                                        let save_in_progress_clone = save_in_progress.clone();
                                        tokio::spawn(async move {
                                            let result = liteclip_replay::app::ClipManager::save_clip(
                                                &config, &buffer,
                                            )
                                            .await;
                                            match result {
                                                Ok(path) => {
                                                    info!("Clip saved: {:?}", path);
                                                    if notifications_enabled {
                                                        let _ = platform_handle_clone.show_notification(
                                                            "Clip Saved",
                                                            &format!(
                                                                "Saved to {:?}",
                                                                path.file_name().unwrap_or_default()
                                                            ),
                                                        );
                                                    }
                                                }
                                                Err(e) => {
                                                    error!("Failed to save clip: {:#}", e);
                                                    if notifications_enabled {
                                                        let _ = platform_handle_clone.show_notification(
                                                            "Save Failed",
                                                            &format!("{:#}", e),
                                                        );
                                                    }
                                                }
                                            }
                                            save_in_progress_clone.store(false, Ordering::SeqCst);
                                        });
                                    }
                                    liteclip_replay::platform::TrayEvent::Exit => {
                                        info!("Tray: Exit selected");
                                        break;
                                    }
                                    liteclip_replay::platform::TrayEvent::Restart => {
                                        info!("Tray: Restart selected");
                                        should_restart = true;
                                        break;
                                    }
                                    liteclip_replay::platform::TrayEvent::OpenSettings => {
                                        info!("Tray: Open Settings selected");
                                        liteclip_replay::gui::run_settings_gui(tokio_tx.clone());
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
                            liteclip_replay::platform::AppEvent::Restart => {
                                info!("Restart signal received");
                                should_restart = true;
                                break;
                            }
                        }
                    }
                    Ok(None) => {
                        info!("Event channel closed");
                        break;
                    }
                    Err(_) => {
                        // Timeout tick: poll worker health and fail-closed when needed.
                        let mut state = app_state.write().await;
                        match state.enforce_pipeline_health().await {
                            Ok(Some(reason)) => {
                                error!("Recording stopped due to fatal pipeline error: {}", reason);
                                let _ = platform_handle.update_recording_state(false);
                                if state.config().general.notifications {
                                    let _ = platform_handle.show_notification(
                                        "Recording Stopped",
                                        &format!("Pipeline error: {}", reason),
                                    );
                                }
                            }
                            Ok(None) => {}
                            Err(e) => {
                                error!("Failed to enforce pipeline health: {}", e);
                            }
                        }
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
    info!("Shutting down (should_restart={})", should_restart);

    // For restart: spawn the new process IMMEDIATELY so the user sees it start
    // right away while we do cleanup in the background.
    if should_restart {
        info!("Spawning new instance for restart...");
        match std::env::current_exe() {
            Ok(current_exe) => {
                let args: Vec<String> = std::env::args().skip(1).collect();
                match Command::new(&current_exe)
                    .args(&args)
                    .creation_flags(CREATE_NO_WINDOW)
                    .spawn()
                {
                    Ok(_) => info!("New instance spawned successfully"),
                    Err(e) => error!("Failed to spawn new instance: {}", e),
                }
            }
            Err(e) => error!("Failed to get current executable path for restart: {}", e),
        }
    }

    // Signal platform thread to quit first — this drops the tray icon immediately
    // giving the user instant visual feedback that Exit/Restart was acknowledged.
    info!("Stopping platform thread (quit signal)...");
    if let Err(e) = platform_handle.quit() {
        warn!("Failed to send quit to platform thread: {}", e);
    }

    // Watchdog: force-exit if cleanup hangs beyond 2 seconds total.
    let (shutdown_done_tx, shutdown_done_rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        if shutdown_done_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .is_err()
        {
            error!("Shutdown timed out after 2s - forcing exit");
            std::process::exit(0);
        }
    });

    {
        let mut state = app_state.write().await;
        info!("Stopping recording pipeline...");
        match tokio::time::timeout(
            tokio::time::Duration::from_millis(1500),
            state.stop_recording(),
        )
        .await
        {
            Ok(Ok(())) => info!("Recording pipeline stopped"),
            Ok(Err(e)) => warn!("Error stopping recording: {}", e),
            Err(_) => warn!("Timed out stopping recording pipeline"),
        }
    }

    // Wait for platform thread (should be quick; we sent Quit above).
    platform_handle.join().ok();

    let _ = shutdown_done_tx.send(());
    info!("LiteClip Replay stopped");

    // Use process::exit for a clean, fast termination — avoids potential hangs
    // in tokio runtime teardown or lingering async drop paths.
    std::process::exit(0);
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
