//! LiteClip Replay - Application Entry Point
//!
//! This is the main entry point for LiteClip Replay. The application runs as a
//! background process with system tray integration, providing screen recording
//! with replay buffer functionality.
//!
//! # Architecture
//!
//! The entry point performs the following initialization sequence:
//!
//! 1. Initialize FFmpeg libraries
//! 2. Set up logging via tracing
//! 3. Configure Windows timer resolution for precise frame timing
//! 4. Create a job object for child process management
//! 5. Load configuration from `%APPDATA%\liteclip-replay\config.toml`
//! 6. Initialize application state and recording pipeline
//! 7. Spawn platform thread for hotkeys and tray
//! 8. Enter main event loop
//!
//! # Threading Model
//!
//! - **Main Thread**: Async runtime with tokio, handles event loop
//! - **Platform Thread**: Windows message loop for hotkeys and tray
//! - **Capture Thread**: DXGI frame acquisition (spawned by pipeline)
//! - **Encode Thread**: Video/audio encoding (spawned by pipeline)
//!
//! # Exit Handling
//!
//! The application supports graceful shutdown via:
//! - Ctrl+C signal
//! - Tray menu "Exit" option
//! - Tray menu "Restart" option (spawns new instance)
//!
//! A 2-second watchdog ensures cleanup doesn't hang indefinitely.

// Hide console window on Windows in release, but keep console in debug so logs are visible.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use anyhow::{Context, Result};
use liteclip_replay::{app::AppState, config::Config, detection::GameDetector};
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
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x01000000;

/// Guard for Windows multimedia timer resolution.
///
/// Windows defaults to 15.6ms timer resolution, which is too coarse for
/// precise frame timing at 60+ FPS. This guard requests a higher resolution
/// (typically 1ms) and automatically restores the original resolution on drop.
///
/// # Example
///
/// ```ignore
/// let _guard = TimerResolutionGuard::new(1);
/// // Timer resolution is now 1ms
/// // When _guard goes out of scope, resolution is restored
/// ```
#[cfg(windows)]
struct TimerResolutionGuard {
    /// Whether the timer resolution change was successful.
    active: bool,
    /// The requested resolution in milliseconds.
    period_ms: u32,
}

#[cfg(windows)]
impl TimerResolutionGuard {
    /// Creates a new timer resolution guard.
    ///
    /// Requests the specified timer resolution from Windows via `timeBeginPeriod`.
    /// The resolution is automatically restored when the guard is dropped.
    ///
    /// # Arguments
    ///
    /// * `period_ms` - The requested timer resolution in milliseconds (typically 1).
    ///
    /// # Example
    ///
    /// ```ignore
    /// let guard = TimerResolutionGuard::new(1);
    /// if guard.active {
    ///     // Timer resolution successfully set to 1ms
    /// }
    /// ```
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

/// Application entry point.
///
/// Initializes and runs the LiteClip Replay application. The function:
///
/// 1. Initializes FFmpeg libraries for encoding/muxing
/// 2. Configures environment variables for Vulkan/FFmpeg
/// 3. Sets up structured logging with tracing
/// 4. Configures Windows timer resolution for precise frame timing
/// 5. Creates a job object to manage child processes (FFmpeg)
/// 6. Loads configuration from disk or uses defaults
/// 7. Starts the recording pipeline
/// 8. Enters the main event loop handling hotkeys and tray events
///
/// # Exit Codes
///
/// The application exits with code 0 on normal termination.
/// Restart is handled by spawning a new process before exit.
///
/// # Errors
///
/// Returns an error if critical initialization fails (FFmpeg, configuration, etc.).
#[tokio::main]
async fn main() -> Result<()> {
    // Initialize FFmpeg
    liteclip_replay::encode::init_ffmpeg().context("Failed to initialize FFmpeg")?;

    // Suppress all Vulkan loader output (prints directly to stderr from C code)
    std::env::set_var("VK_LOADER_DEBUG", "none");
    std::env::set_var("DISABLE_LAYER_AMD_SWITCHABLE_GRAPHICS_1", "1");
    std::env::set_var("DISABLE_VULKAN_OBS_CAPTURE", "1");
    // Disable Vulkan SDK validation layer for release-like behavior
    std::env::set_var("VK_LAYER_PATH", "");

    // Initialize compact logger with filters to suppress noisy dependencies
    let filter = tracing_subscriber::filter::EnvFilter::new("info,wgpu=warn,naga=warn");

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
        use windows::core::PCWSTR;
        use windows::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        use windows::Win32::System::Threading::GetCurrentProcess;

        unsafe {
            if let Ok(job) = CreateJobObjectW(None, PCWSTR::null()) {
                let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                info.BasicLimitInformation.LimitFlags =
                    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
                let _ = SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                let _ = AssignProcessToJobObject(job, GetCurrentProcess());
                // We purposefully leak the job handle so it lives as long as the process.
                let _ = job;
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
        "Config: {}s @ {} Mbps, {} FPS, codec=HEVC, encoder={:?}, preset={:?}, rc={:?}, q={:?}, replay-est={} MB, buffer-cap={} MB",
        config.general.replay_duration_secs,
        config.video.bitrate_mbps,
        config.video.framerate,
        config.video.encoder,
        config.video.quality_preset,
        config.video.rate_control,
        config.video.quality_value,
        config.estimated_replay_storage_mb(),
        config.effective_replay_memory_limit_mb()
    );

    // Apply auto-start registry entry based on config
    match liteclip_replay::platform::autostart::set_autostart(
        config.general.auto_start_with_windows,
    ) {
        Ok(()) => info!(
            "Auto-start set to {}",
            config.general.auto_start_with_windows
        ),
        Err(e) => warn!("Failed to configure auto-start: {}", e),
    }
    if config.general.auto_detect_game {
        info!("Game detection enabled");
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

    // Initialize game detector if enabled
    let game_detector = if config.general.auto_detect_game {
        let detector = GameDetector::new();
        detector.start();
        info!("Game detector started");
        Some(Arc::new(detector))
    } else {
        None
    };

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
                                                spawn_save_clip_task(
                                                    &app_state,
                                                    &platform_handle,
                                                    &save_in_progress,
                                                    &game_detector,
                                                )
                                                .await;
                                            }
                                            liteclip_replay::platform::HotkeyAction::ToggleRecording => {
                                                info!("Hotkey: toggle recording");
                                                let mut state = app_state.write().await;
                                                if state.is_recording() {
                                                    if let Err(e) = state.stop_recording().await {
                                                        error!("Failed to stop recording: {}", e);
                                                    } else {
                                                        let _ = platform_handle.update_recording_state(false);
                                                    }
                                                } else if let Err(e) = state.start_recording().await {
                                                    error!("Failed to start recording: {}", e);
                                                } else {
                                                    let _ = platform_handle.update_recording_state(true);
                                                }
                                            }
                                            liteclip_replay::platform::HotkeyAction::Screenshot => {
                                                info!("Hotkey: screenshot (not implemented)");
                                                warn!("Screenshot feature not yet implemented");
                                            }
                                            liteclip_replay::platform::HotkeyAction::OpenGallery => {
                                                info!("Hotkey: open gallery");
                                                liteclip_replay::gui::show_gallery_gui(tokio_tx.clone());
                                            }
                                        }
                                    }
                                    liteclip_replay::platform::AppEvent::Tray(tray_event) => {
                                        match tray_event {
        liteclip_replay::platform::TrayEvent::SaveClip => {
                                                info!("Tray: Save Clip selected");
                                                spawn_save_clip_task(
                                                    &app_state,
                                                    &platform_handle,
                                                    &save_in_progress,
                                                    &game_detector,
                                                )
                                                .await;
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
                                                 liteclip_replay::gui::show_settings_gui(tokio_tx.clone());
                                             }
                                             liteclip_replay::platform::TrayEvent::OpenGallery => {
                                                 info!("Tray: Open Gallery selected");
                                                 liteclip_replay::gui::show_gallery_gui(tokio_tx.clone());
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
                                    liteclip_replay::platform::AppEvent::ConfigUpdated(new_config) => {
                                        info!("ConfigUpdated event received from settings GUI");
                                        let mut state = app_state.write().await;
                                        match state.apply_config((*new_config).clone()).await {
                                            Ok(needs_hotkey_reregister) => {
                                                if let Err(e) = state.config().save_sync() {
                                                    error!("Failed to save config: {}", e);
                                                }
                                                if needs_hotkey_reregister {
                                                    let hk = hotkey_config_from_config(state.config());
                                                    if let Err(e) = platform_handle.re_register_hotkeys(hk) {
                                                        error!("Failed to re-register hotkeys: {}", e);
                                                    }
        }
                                                let _ = platform_handle.update_recording_state(true);
                                            }
                                            Err(e) => {
                                                error!("Failed to apply config: {}", e);
                                            }
                                        }
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

    // Restart logic moved to the very end to ensure all resources are released.

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

    // For restart: spawn the new process after everything else is cleaned up
    // to avoid resource conflicts (GPU, Audio, Tray icon, etc.)
    if should_restart {
        info!("Spawning new instance for restart...");
        match std::env::current_exe() {
            Ok(current_exe) => {
                let args: Vec<String> = std::env::args().skip(1).collect();
                match Command::new(&current_exe)
                    .args(&args)
                    .creation_flags(CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB)
                    .spawn()
                {
                    Ok(_) => info!("New instance spawned successfully"),
                    Err(e) => error!("Failed to spawn new instance: {}", e),
                }
            }
            Err(e) => error!("Failed to get current executable path for restart: {}", e),
        }
    }

    // Use process::exit for a clean, fast termination — avoids potential hangs
    // in tokio runtime teardown or lingering async drop paths.
    std::process::exit(0);
}

/// Extracts hotkey configuration from the application config.
///
/// # Arguments
///
/// * `config` - Reference to the application configuration.
///
/// # Returns
///
/// A [`HotkeyConfig`] struct containing hotkey strings for all actions.
fn hotkey_config_from_config(config: &Config) -> liteclip_replay::platform::HotkeyConfig {
    config.hotkeys.clone()
}

/// Spawns an async task to save the current replay buffer to disk.
///
/// This function provides concurrency protection to ensure only one save
/// operation runs at a time. If a save is already in progress, the request
/// is ignored.
///
/// # Arguments
///
/// * `app_state` - Shared application state containing the replay buffer.
/// * `platform_handle` - Handle to the platform layer.
/// * `save_in_progress` - Atomic flag for concurrency control.
/// * `game_detector` - Optional game detector for organizing clips by game.
///
/// # Example
///
/// ```ignore
/// spawn_save_clip_task(
///     &app_state,
///     &platform_handle,
///     &save_in_progress,
///     &game_detector,
/// ).await;
/// ```
async fn spawn_save_clip_task(
    app_state: &Arc<RwLock<AppState>>,
    _platform_handle: &Arc<liteclip_replay::platform::PlatformHandle>,
    save_in_progress: &Arc<AtomicBool>,
    game_detector: &Option<Arc<GameDetector>>,
) {
    if save_in_progress
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        info!("Save request ignored: clip save already in progress");
        return;
    }

    let (config, buffer) = app_state.read().await.save_context();
    let save_in_progress_clone = save_in_progress.clone();

    let game_name = game_detector.as_ref().and_then(|d| {
        let app = d.get_detected_app();
        if app.is_game {
            Some(app.folder_name.clone())
        } else {
            None
        }
    });

    tokio::spawn(async move {
        let result =
            liteclip_replay::app::ClipManager::save_clip(&config, &buffer, game_name.as_deref())
                .await;

        match result {
            Ok(path) => {
                info!("Clip saved: {:?}", path);
                let filename = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "clip".to_string());
                liteclip_replay::gui::show_toast(
                    liteclip_replay::gui::ToastKind::Success,
                    format!("Clip saved: {}", filename),
                );
            }
            Err(e) => {
                error!("Failed to save clip: {:#}", e);
                liteclip_replay::gui::show_toast(
                    liteclip_replay::gui::ToastKind::Error,
                    format!("Failed to save clip: {}", e),
                );
            }
        }

        save_in_progress_clone.store(false, Ordering::SeqCst);
    });
}
