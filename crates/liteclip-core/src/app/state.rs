use crate::{
    app::RecordingPipeline, buffer::ReplayBuffer, capture::audio::AudioLevelMonitor,
    config::Config, host::CoreHost,
};
use anyhow::Result;
use std::sync::Arc;
use tracing::{error, info, warn};

/// Application state manager.
///
/// Central coordinator for LiteClip Replay, managing:
/// - Configuration
/// - Replay buffer
/// - Recording pipeline
///
/// # Thread Safety
///
/// Prefer wrapping this type in `std::sync::Mutex` at the app root and driving lifecycle calls
/// via `tokio::task::spawn_blocking` so encoder thread joins do not block Tokio worker threads.
///
/// Recording lifecycle (`start_recording` / `stop_recording`) is synchronous and may block
/// briefly (e.g. joining the encoder thread).
pub struct AppState {
    /// Application configuration.
    config: Config,
    /// Replay buffer for storing encoded packets.
    buffer: ReplayBuffer,
    /// Recording pipeline for capture → encode → buffer flow.
    pipeline: RecordingPipeline,
    /// Audio level monitor for GUI visualization.
    level_monitor: AudioLevelMonitor,
    /// Optional embedder hooks ([`CoreHost`]).
    host: Option<Arc<dyn CoreHost>>,
}

impl AppState {
    /// Creates a new application state.
    ///
    /// Initializes the replay buffer and recording pipeline with the given
    /// configuration.
    ///
    /// # Arguments
    ///
    /// * `config` - Application configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if buffer initialization fails.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use liteclip_core::config::Config;
    /// use liteclip_core::app::AppState;
    ///
    /// let config = Config::default();
    /// let state = AppState::new(config).unwrap();
    /// ```
    pub fn new(config: Config) -> Result<Self> {
        let buffer = ReplayBuffer::new(&config)?;
        let level_monitor = AudioLevelMonitor::new();
        let mut pipeline = RecordingPipeline::with_defaults();
        pipeline.set_level_monitor(level_monitor.clone());

        Ok(Self {
            config,
            buffer,
            pipeline,
            level_monitor,
            host: None,
        })
    }

    /// Set or clear [`CoreHost`] for **pipeline fatals** ([`CoreHost::on_pipeline_fatal`]).
    ///
    /// Successful clip saves use the `host` argument on [`crate::app::ClipManager::save_clip`]
    /// ([`CoreHost::on_clip_saved`]), not this field.
    pub fn set_core_host(&mut self, host: Option<Arc<dyn CoreHost>>) {
        self.host = host;
    }

    /// Current [`CoreHost`], if any.
    pub fn core_host(&self) -> Option<&Arc<dyn CoreHost>> {
        self.host.as_ref()
    }

    /// Starts the recording pipeline.
    ///
    /// Begins capturing and encoding frames. The replay buffer will start
    /// filling with encoded packets.
    ///
    /// # Errors
    ///
    /// Returns an error if pipeline fails to start.
    pub fn start_recording(&mut self) -> Result<()> {
        self.pipeline.start(&self.config, &self.buffer)
    }

    /// Stops the recording pipeline.
    ///
    /// Stops capture and encoding, releasing all resources. The replay buffer
    /// retains its contents until next start.
    ///
    /// # Errors
    ///
    /// Returns an error if pipeline fails to stop cleanly.
    pub fn stop_recording(&mut self) -> Result<()> {
        self.pipeline.stop()
    }

    /// Polls the recording pipeline for fatal errors (crashed threads, etc.).
    ///
    /// Call periodically from your main loop while recording.
    ///
    /// # Returns
    ///
    /// - `Ok(None)` — healthy, or not running.
    /// - `Ok(Some(reason))` — fatal error; pipeline is stopped.
    /// - `Err(...)` — health check failed.
    ///
    /// If a [`CoreHost`] is installed via [`Self::set_core_host`], a fatal also invokes
    /// [`CoreHost::on_pipeline_fatal`]. Avoid duplicating user-visible handling if you
    /// both match on `Ok(Some(reason))` and implement `on_pipeline_fatal`.
    pub fn enforce_pipeline_health(&mut self) -> Result<Option<String>> {
        let r = self.pipeline.enforce_health()?;
        if let (Some(reason), Some(host)) = (&r, &self.host) {
            host.on_pipeline_fatal(reason);
        }
        Ok(r)
    }

    /// Gets the context needed for clip saving.
    ///
    /// Returns a tuple of (config, buffer) that can
    /// be passed to a background task for clip saving.
    ///
    /// # Returns
    ///
    /// Tuple of:
    /// - Clone of the configuration
    /// - Clone of the replay buffer
    pub fn save_context(&self) -> (Config, ReplayBuffer) {
        (self.config.clone(), self.buffer.clone())
    }

    pub fn replay_buffer_stats(&self) -> crate::buffer::BufferStats {
        self.buffer.stats()
    }

    /// Checks if recording is currently active.
    ///
    /// # Returns
    ///
    /// `true` if the recording pipeline is running.
    pub fn is_recording(&self) -> bool {
        self.pipeline.is_recording()
    }

    /// Applies configuration changes, restarting pipeline if needed.
    ///
    /// Some configuration changes require restarting the recording pipeline
    /// (e.g., encoder changes, resolution changes). This method handles the
    /// restart automatically with rollback on failure.
    ///
    /// # Arguments
    ///
    /// * `new_config` - The new configuration to apply.
    ///
    /// # Returns
    ///
    /// - `Ok(true)` if hotkeys need to be re-registered
    /// - `Ok(false)` if hotkey registration is not needed
    ///
    /// # Errors
    ///
    /// Returns an error if configuration is rejected and rollback fails.
    pub fn apply_config(&mut self, new_config: Config) -> Result<bool> {
        let needs_restart = self.config.requires_pipeline_restart(&new_config);
        let needs_hotkey_reregister = self.config.requires_hotkey_reregister(&new_config);
        let audio_changed = self.config.audio != new_config.audio;

        if needs_restart {
            let old_config = self.config.clone();

            info!("Stopping pipeline for configuration change...");
            self.pipeline.stop()?;

            info!("Restarting pipeline with new configuration...");
            self.config = new_config;
            self.config.validate();

            self.buffer = ReplayBuffer::new(&self.config)?;

            if let Err(e) = self.pipeline.start(&self.config, &self.buffer) {
                error!("Failed to start pipeline with new config: {}", e);
                error!("Rolling back to previous configuration...");

                self.config = old_config;
                self.buffer = ReplayBuffer::new(&self.config)?;

                match self.pipeline.start(&self.config, &self.buffer) {
                    Ok(()) => {
                        warn!("Rollback successful - using previous configuration");
                    }
                    Err(rollback_err) => {
                        error!("CRITICAL: Rollback also failed: {}", rollback_err);
                    }
                }

                return Err(anyhow::anyhow!(
                    "Config rejected: {}. Previous settings restored.",
                    e
                ));
            }
        } else {
            self.config = new_config;
            self.config.validate();

            if audio_changed && self.pipeline.is_recording() {
                self.pipeline.update_audio_config(&self.config.audio);
            }
        }

        Ok(needs_hotkey_reregister)
    }

    /// Gets a reference to the current configuration.
    ///
    /// # Returns
    ///
    /// Reference to the configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Updates audio configuration at runtime without restarting the pipeline.
    ///
    /// This should be called when audio settings (volume levels, etc.) change
    /// but don't require a full pipeline restart.
    pub fn update_audio_config(&self, audio_config: &crate::config::AudioConfig) {
        self.pipeline.update_audio_config(audio_config);
    }

    /// Gets the audio level monitor for visualization.
    pub fn level_monitor(&self) -> &AudioLevelMonitor {
        &self.level_monitor
    }
}
