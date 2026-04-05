//! App harness for end-to-end testing.
//!
//! Provides a controlled environment for testing the full application
//! lifecycle without running the actual main() entry point.

use anyhow::{Context, Result};
use liteclip::app::AppState;
use liteclip::config::Config;
use liteclip::paths::AppDirs;
use liteclip::platform::{HotkeyAction, TrayEvent};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;

use crate::common::test_defaults::fast_test_config;

/// Test harness for controlling the application in e2e tests.
///
/// The harness provides a simplified interface for:
/// - Creating an AppState with test configuration
/// - Controlling the recording lifecycle
/// - Simulating user interactions (hotkeys, tray)
/// - Verifying output files
///
/// # Example
///
/// ```
/// #[tokio::test]
/// async fn test_recording() -> Result<()> {
///     let mut harness = AppHarness::new().await?;
///     harness.start_recording().await?;
///     // ... test logic ...
///     harness.stop_recording().await?;
///     harness.shutdown().await?;
///     Ok(())
/// }
/// ```
pub struct AppHarness {
    app_state: Arc<Mutex<AppState>>,
    _temp_dir: TempDir,
    clips_dir: PathBuf,
    config: Config,
    recording: Arc<AtomicBool>,
}

impl AppHarness {
    /// Creates a new test harness with default test configuration.
    ///
    /// Sets up:
    /// - Temporary directory for config and clips
    /// - Fast test configuration (10s replay, 720p, software encoder)
    /// - AppState initialized but not recording
    pub async fn new() -> Result<Self> {
        Self::with_config(fast_test_config()).await
    }

    /// Creates a new test harness with a custom configuration.
    ///
    /// The config's save directory will be overridden to use a temp directory.
    pub async fn with_config(mut config: Config) -> Result<Self> {
        // Create temp directory for this test
        let temp_dir = TempDir::new().context("Failed to create temp directory")?;
        let clips_dir = temp_dir.path().join("clips");
        std::fs::create_dir_all(&clips_dir)?;

        // Override config paths to use temp directory
        config.general.save_directory = clips_dir.to_string_lossy().to_string();

        // Create AppDirs pointing to temp config (needed for proper path initialization)
        let config_path = temp_dir.path().join("config.toml");
        let _ = AppDirs::with_config_file(config_path, "liteclip-e2e")
            .context("Failed to create AppDirs")?;

        // Initialize AppState (may need ffmpeg)
        let app_state = AppState::new(config.clone()).context("Failed to create AppState")?;

        Ok(Self {
            app_state: Arc::new(Mutex::new(app_state)),
            _temp_dir: temp_dir,
            clips_dir,
            config,
            recording: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Returns the directory where clips will be saved.
    pub fn clips_dir(&self) -> &Path {
        &self.clips_dir
    }

    /// Returns the current configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Starts recording if not already recording.
    ///
    /// Returns an error if the recording fails to start.
    pub async fn start_recording(&self) -> Result<()> {
        if self.recording.load(Ordering::SeqCst) {
            return Ok(());
        }

        let app_state = self.app_state.clone();
        tokio::task::spawn_blocking(move || {
            let mut state = app_state
                .lock()
                .map_err(|e| anyhow::anyhow!("Mutex poisoned: {}", e))?;
            state.start_recording()
        })
        .await
        .context("Task join error")?
        .context("Failed to start recording")?;

        self.recording.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Stops recording if currently recording.
    ///
    /// Returns an error if the recording fails to stop.
    pub async fn stop_recording(&self) -> Result<()> {
        if !self.recording.load(Ordering::SeqCst) {
            return Ok(());
        }

        let app_state = self.app_state.clone();
        tokio::task::spawn_blocking(move || {
            let mut state = app_state
                .lock()
                .map_err(|e| anyhow::anyhow!("Mutex poisoned: {}", e))?;
            state.stop_recording()
        })
        .await
        .context("Task join error")?
        .context("Failed to stop recording")?;

        self.recording.store(false, Ordering::SeqCst);
        Ok(())
    }

    /// Checks if the recording pipeline is currently active.
    pub async fn is_recording(&self) -> bool {
        let app_state = self.app_state.clone();
        tokio::task::spawn_blocking(move || {
            let state = app_state.lock().ok()?;
            Some(state.is_recording())
        })
        .await
        .ok()
        .flatten()
        .unwrap_or(false)
    }

    /// Simulates a hotkey action being pressed.
    ///
    /// Note: This directly invokes the app logic rather than simulating
    /// actual Windows hotkey messages, making tests more reliable.
    pub async fn simulate_hotkey(&self, action: HotkeyAction) -> Result<()> {
        let app_state = self.app_state.clone();

        let _ = tokio::task::spawn_blocking(move || {
            let state = app_state
                .lock()
                .map_err(|e| anyhow::anyhow!("Mutex poisoned: {}", e))?;

            match action {
                HotkeyAction::SaveClip => {
                    // Return config and buffer to save outside spawn_blocking
                    let (cfg, buffer) = state.save_context();
                    Ok::<_, anyhow::Error>((Some(cfg), Some(buffer), None::<()>))
                }
                HotkeyAction::ToggleRecording => {
                    // Toggle handled after spawn_blocking
                    Ok((None, None, Some(())))
                }
                _ => {
                    // Other actions not yet implemented
                    Ok((None, None, None))
                }
            }
        })
        .await
        .context("Task join error")?;

        match action {
            HotkeyAction::SaveClip => {
                let (cfg, buffer) = self
                    .app_state
                    .lock()
                    .map_err(|e| anyhow::anyhow!("Mutex poisoned: {}", e))?
                    .save_context();
                liteclip::app::ClipManager::save_clip(&cfg, &buffer, None, None).await?;
            }
            HotkeyAction::ToggleRecording => {
                if self.recording.load(Ordering::SeqCst) {
                    self.stop_recording().await?;
                } else {
                    self.start_recording().await?;
                }
            }
            _ => {}
        }

        Ok(())
    }

    /// Simulates a tray menu event.
    pub async fn simulate_tray_event(&self, event: TrayEvent) -> Result<()> {
        match event {
            TrayEvent::SaveClip => {
                self.simulate_hotkey(HotkeyAction::SaveClip).await?;
            }
            TrayEvent::Exit => {
                // Shutdown will be handled by the test harness
                // This simulates the user clicking Exit in the tray menu
            }
            TrayEvent::ToggleRecording => {
                self.simulate_hotkey(HotkeyAction::ToggleRecording).await?;
            }
            TrayEvent::OpenSettings => {
                // Settings window would open - not implemented in harness
                // Tests should verify this doesn't cause panic
            }
            TrayEvent::OpenGallery => {
                // Gallery window would open - not implemented in harness
                // Tests should verify this doesn't cause panic
            }
            _ => {
                // Unknown events are silently ignored
            }
        }
        Ok(())
    }

    /// Saves a clip using the current buffer contents.
    ///
    /// Returns the path to the saved clip file.
    pub async fn save_clip(&self) -> Result<PathBuf> {
        let app_state = self.app_state.clone();

        let (config, buffer) = tokio::task::spawn_blocking(move || {
            let state = app_state
                .lock()
                .map_err(|e| anyhow::anyhow!("Mutex poisoned: {}", e))?;
            let (config, buffer) = state.save_context();
            Ok::<_, anyhow::Error>((config, buffer))
        })
        .await
        .context("Task join error")??;

        // Perform async save outside spawn_blocking
        let clip_path = liteclip::app::ClipManager::save_clip(&config, &buffer, None, None).await?;

        Ok(clip_path)
    }

    /// Performs a pipeline health check.
    ///
    /// Returns `Ok(None)` if healthy, `Ok(Some(reason))` if stopped.
    pub async fn check_pipeline_health(&self) -> Result<Option<String>> {
        let app_state = self.app_state.clone();
        tokio::task::spawn_blocking(move || {
            let mut state = app_state
                .lock()
                .map_err(|e| anyhow::anyhow!("Mutex poisoned: {}", e))?;
            state.enforce_pipeline_health()
        })
        .await
        .context("Task join error")?
        .context("Health check failed")
    }

    /// Shuts down the harness, stopping recording if active.
    ///
    /// This should be called at the end of each test to ensure cleanup.
    pub async fn shutdown(self) -> Result<()> {
        if self.recording.load(Ordering::SeqCst) {
            let _ = self.stop_recording().await;
        }

        // AppState will be dropped, cleaning up resources
        // TempDir will be deleted on drop
        Ok(())
    }

    /// Lists all clip files in the clips directory.
    pub fn list_clips(&self) -> Result<Vec<PathBuf>> {
        let mut clips = Vec::new();
        if self.clips_dir.exists() {
            for entry in std::fs::read_dir(&self.clips_dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().map(|e| e == "mp4").unwrap_or(false) {
                    clips.push(path);
                }
            }
        }
        Ok(clips)
    }

    /// Waits for a clip file to appear in the clips directory.
    ///
    /// Returns the path to the first matching clip, or an error if timeout expires.
    pub async fn wait_for_clip(&self, timeout: Duration) -> Result<PathBuf> {
        let clips_dir = self.clips_dir.clone();
        let start = std::time::Instant::now();

        loop {
            if start.elapsed() > timeout {
                anyhow::bail!("Timeout waiting for clip file");
            }

            // Check for mp4 files
            if let Ok(entries) = std::fs::read_dir(&clips_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().map(|e| e == "mp4").unwrap_or(false) {
                        // Give the file a moment to finish writing
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        return Ok(path);
                    }
                }
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

/// Builder for creating test harnesses with custom settings.
pub struct HarnessBuilder {
    config: Option<Config>,
}

impl HarnessBuilder {
    /// Creates a new builder with default settings.
    pub fn new() -> Self {
        Self { config: None }
    }

    /// Sets a custom configuration.
    pub fn with_config(mut self, config: Config) -> Self {
        self.config = Some(config);
        self
    }

    /// Builds the harness.
    pub async fn build(self) -> Result<AppHarness> {
        match self.config {
            Some(config) => AppHarness::with_config(config).await,
            None => AppHarness::new().await,
        }
    }
}

impl Default for HarnessBuilder {
    fn default() -> Self {
        Self::new()
    }
}
