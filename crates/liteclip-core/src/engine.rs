//! Thin facade over [`AppState`](crate::app::AppState) and [`AppDirs`](crate::paths::AppDirs).
//!
//! **Host callbacks:** Successful clip writes can pass [`CoreHost`] to [`ReplayEngine::save_clip`].
//! Pipeline fatals are delivered only via [`AppState::set_core_host`] on
//! [`ReplayEngine::state_mut`] ([`CoreHost::on_pipeline_fatal`]), not through [`ReplayEngine::save_clip`].

use crate::app::{AppState, ClipManager};
use crate::config::Config;
use crate::host::CoreHost;
use crate::paths::AppDirs;
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;

/// Recording session with resolved [`AppDirs`] for configuration defaults.
///
/// To receive pipeline fatal notifications, call [`AppState::set_core_host`] on
/// [`Self::state_mut`]. The `host` argument on [`Self::save_clip`] is only for
/// [`CoreHost::on_clip_saved`].
pub struct ReplayEngine {
    dirs: AppDirs,
    state: AppState,
}

impl ReplayEngine {
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

    pub fn start_recording(&mut self) -> Result<()> {
        self.state.start_recording()
    }

    pub fn stop_recording(&mut self) -> Result<()> {
        self.state.stop_recording()
    }

    pub fn enforce_pipeline_health(&mut self) -> Result<Option<String>> {
        self.state.enforce_pipeline_health()
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
        let (config, buffer) = self.state.save_context();
        ClipManager::save_clip(&config, &buffer, game_name, host).await
    }
}
