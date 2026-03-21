//! Thin facade over [`AppState`](crate::app::AppState) and [`AppDirs`](crate::paths::AppDirs).
//!
//! **Host callbacks:** Use the same [`Arc`] for both paths when you want one integration object:
//! - Call [`ReplayEngine::set_core_host`] (or [`AppState::set_core_host`] via [`Self::state_mut`])
//!   so [`CoreHost::on_pipeline_fatal`] runs when the pipeline stops with a fatal error.
//! - Pass `Some(host)` to [`ReplayEngine::save_clip`] so [`CoreHost::on_clip_saved`] runs after a
//!   successful export. [`save_clip`](ReplayEngine::save_clip) is `async` and uses Tokio internally;
//!   run it from a Tokio runtime (see crate root docs).

use crate::app::{AppState, ClipManager};
use crate::config::Config;
use crate::error::Result;
use crate::host::CoreHost;
use crate::paths::AppDirs;
use std::path::PathBuf;
use std::sync::Arc;

/// Builder for `ReplayEngine`.
pub struct ReplayEngineBuilder {
    dirs: AppDirs,
    config: Option<Config>,
    host: Option<Arc<dyn CoreHost>>,
}

impl ReplayEngineBuilder {
    pub fn new(dirs: AppDirs) -> Self {
        Self {
            dirs,
            config: None,
            host: None,
        }
    }

    pub fn with_config(mut self, config: Config) -> Self {
        self.config = Some(config);
        self
    }

    pub fn with_host(mut self, host: Arc<dyn CoreHost>) -> Self {
        self.host = Some(host);
        self
    }

    pub fn build(self) -> Result<ReplayEngine> {
        let config = self
            .config
            .unwrap_or_else(|| Config::default_with_dirs(&self.dirs));
        let mut state = AppState::new(config)?;
        state.set_core_host(self.host);
        Ok(ReplayEngine {
            dirs: self.dirs,
            state,
        })
    }
}

/// Recording session with resolved [`AppDirs`] for configuration defaults.
///
/// Prefer [`Self::set_core_host`] / [`Self::core_host`] for pipeline fatals, or [`Self::state_mut`]
/// when you need full [`AppState`] access. The `host` argument on [`Self::save_clip`] is only for
/// [`CoreHost::on_clip_saved`].
pub struct ReplayEngine {
    dirs: AppDirs,
    state: AppState,
}

impl ReplayEngine {
    /// Start building a `ReplayEngine`.
    pub fn builder(dirs: AppDirs) -> ReplayEngineBuilder {
        ReplayEngineBuilder::new(dirs)
    }

    /// Create engine from an existing [`Config`] and [`AppDirs`].
    pub fn new(config: Config, dirs: AppDirs) -> Result<Self> {
        Ok(Self {
            dirs,
            state: AppState::new(config)?,
        })
    }

    /// [`Config::default_with_dirs`] then [`Self::new`].
    pub fn with_default_config(dirs: AppDirs) -> Result<Self> {
        let config = Config::default_with_dirs(&dirs);
        Self::new(config, dirs)
    }

    /// Application directory layout.
    pub fn dirs(&self) -> &AppDirs {
        &self.dirs
    }

    pub fn state(&self) -> &AppState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut AppState {
        &mut self.state
    }

    /// Set the [`CoreHost`] used for **pipeline fatals** ([`CoreHost::on_pipeline_fatal`]).
    ///
    /// Same as [`AppState::set_core_host`]; provided on the facade so embedders rarely need
    /// [`Self::state_mut`] only for host wiring.
    pub fn set_core_host(&mut self, host: Option<Arc<dyn CoreHost>>) {
        self.state.set_core_host(host);
    }

    /// Current [`CoreHost`] for pipeline fatals, if any ([`AppState::core_host`]).
    pub fn core_host(&self) -> Option<&Arc<dyn CoreHost>> {
        self.state.core_host()
    }

    pub fn start_recording(&mut self) -> Result<()> {
        Ok(self.state.start_recording()?)
    }

    pub fn stop_recording(&mut self) -> Result<()> {
        Ok(self.state.stop_recording()?)
    }

    pub fn enforce_pipeline_health(&mut self) -> Result<Option<String>> {
        Ok(self.state.enforce_pipeline_health()?)
    }

    /// Save the replay buffer; see [`ClipManager::save_clip`].
    ///
    /// `host` is only used for [`CoreHost::on_clip_saved`]. Pipeline errors use
    /// [`AppState::set_core_host`] on [`Self::state_mut`].
    pub async fn save_clip(
        &self,
        game_name: Option<&str>,
        host: Option<Arc<dyn CoreHost>>,
    ) -> Result<PathBuf> {
        let (config, buffer, webcam_buffer) = self.state.save_context();
        let path =
            ClipManager::save_clip(&config, &buffer, webcam_buffer.as_ref(), game_name, host)
                .await?;
        Ok(path)
    }
}
