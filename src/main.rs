//! LiteClip Replay - CLI Entry Point (Phase 1)
//!
//! Minimal CLI-only interface for testing the core recording pipeline.
//! GUI will be added in Phase 2.

use anyhow::Result;
use liteclip_replay::{app::AppState, config::Config};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize compact logger: short, readable output with INFO as default.
    tracing_subscriber::fmt()
        .compact()
        .with_target(false)
        .without_time()
        .with_level(true)
        .with_max_level(tracing::Level::INFO)
        .init();

    let version = env!("CARGO_PKG_VERSION");
    println!("LiteClip Replay v{}", version);
    info!("LiteClip Replay {} starting", version);

    #[cfg(not(feature = "ffmpeg"))]
    warn!(
        "Built without FFmpeg support. Saved clips cannot be muxed to playable MP4. Rebuild with `cargo run --features ffmpeg`."
    );

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

    // Start the platform message loop for hotkeys
    let hotkey_config = liteclip_replay::platform::HotkeyConfig::from(&config);
    let (platform_handle, event_rx) =
        liteclip_replay::platform::spawn_platform_thread(hotkey_config)?;

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

    // Main event loop (CLI only - just handle hotkey events)
    loop {
        tokio::select! {
            // Handle platform events (hotkeys)
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
