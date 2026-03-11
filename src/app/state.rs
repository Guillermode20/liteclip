use crate::{app::RecordingPipeline, buffer::ReplayBuffer, config::Config};
use anyhow::Result;
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
/// Uses `tokio::RwLock` for async-aware concurrent access.
/// Multiple readers can access state simultaneously; writers get exclusive access.
pub struct AppState {
    /// Application configuration.
    config: Config,
    /// Replay buffer for storing encoded packets.
    buffer: ReplayBuffer,
    /// Recording pipeline for capture → encode → buffer flow.
    pipeline: RecordingPipeline,
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
    /// use liteclip_replay::config::Config;
    /// use liteclip_replay::app::AppState;
    ///
    /// let config = Config::default();
    /// let state = AppState::new(config).unwrap();
    /// ```
    pub fn new(config: Config) -> Result<Self> {
        let buffer = ReplayBuffer::new(&config)?;

        Ok(Self {
            config,
            buffer,
            pipeline: RecordingPipeline::new(),
        })
    }

    /// Starts the recording pipeline.
    ///
    /// Begins capturing and encoding frames. The replay buffer will start
    /// filling with encoded packets.
    ///
    /// # Errors
    ///
    /// Returns an error if pipeline fails to start.
    pub async fn start_recording(&mut self) -> Result<()> {
        self.pipeline.start(&self.config, &self.buffer).await
    }

    /// Stops the recording pipeline.
    ///
    /// Stops capture and encoding, releasing all resources. The replay buffer
    /// retains its contents until next start.
    ///
    /// # Errors
    ///
    /// Returns an error if pipeline fails to stop cleanly.
    pub async fn stop_recording(&mut self) -> Result<()> {
        self.pipeline.stop().await
    }

    /// Enforces pipeline health by checking for errors.
    ///
    /// Polls the pipeline for fatal errors (crashes, dead threads) and
    /// returns the error reason if the pipeline has failed.
    ///
    /// # Returns
    ///
    /// - `Ok(Some(reason))` if pipeline has failed with the given reason
    /// - `Ok(None)` if pipeline is healthy
    /// - `Err(...)` if health check itself failed
    pub async fn enforce_pipeline_health(&mut self) -> Result<Option<String>> {
        self.pipeline.enforce_health().await
    }

    /// Gets the context needed for clip saving.
    ///
    /// Returns a tuple of (config, buffer, notifications_enabled) that can
    /// be passed to a background task for clip saving.
    ///
    /// # Returns
    ///
    /// Tuple of:
    /// - Clone of the configuration
    /// - Clone of the replay buffer
    /// - Whether notifications are enabled
    pub fn save_context(&self) -> (Config, ReplayBuffer, bool) {
        (
            self.config.clone(),
            self.buffer.clone(),
            self.config.general.notifications,
        )
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
    pub async fn apply_config(&mut self, new_config: Config) -> Result<bool> {
        let needs_restart = self.config.requires_pipeline_restart(&new_config);
        let needs_hotkey_reregister = self.config.requires_hotkey_reregister(&new_config);

        if needs_restart {
            let old_config = self.config.clone();

            info!("Stopping pipeline for configuration change...");
            self.pipeline.stop().await?;

            info!("Restarting pipeline with new configuration...");
            self.config = new_config;
            self.config.validate();

            self.buffer = ReplayBuffer::new(&self.config)?;

            if let Err(e) = self.pipeline.start(&self.config, &self.buffer).await {
                error!("Failed to start pipeline with new config: {}", e);
                error!("Rolling back to previous configuration...");

                self.config = old_config;
                self.buffer = ReplayBuffer::new(&self.config)?;

                match self.pipeline.start(&self.config, &self.buffer).await {
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
}
