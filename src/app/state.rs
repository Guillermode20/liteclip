use crate::{
    app::{ClipManager, RecordingLifecycle, RecordingPipeline},
    buffer::{BufferStats, ReplayBuffer},
    config::Config,
};
use anyhow::Result;
use std::path::PathBuf;
use tracing::{info, warn};

pub struct AppState {
    config: Config,
    buffer: ReplayBuffer,
    pipeline: RecordingPipeline,
}

impl AppState {
    pub fn new(config: Config) -> Result<Self> {
        let buffer = ReplayBuffer::new(&config)?;

        Ok(Self {
            config,
            buffer,
            pipeline: RecordingPipeline::new(),
        })
    }

    pub async fn start_recording(&mut self) -> Result<()> {
        self.pipeline.start(&self.config, &self.buffer).await
    }

    pub async fn stop_recording(&mut self) -> Result<()> {
        self.pipeline.stop().await
    }

    pub async fn enforce_pipeline_health(&mut self) -> Result<Option<String>> {
        self.pipeline.enforce_health().await
    }

    pub async fn save_clip(&self) -> Result<PathBuf> {
        ClipManager::save_clip(&self.config, &self.buffer).await
    }

    pub fn save_context(&self) -> (Config, ReplayBuffer, bool) {
        (
            self.config.clone(),
            self.buffer.clone(),
            self.config.general.notifications,
        )
    }

    pub fn buffer_stats(&self) -> BufferStats {
        self.buffer.stats()
    }

    pub fn is_recording(&self) -> bool {
        self.pipeline.is_recording()
    }

    pub fn lifecycle(&self) -> RecordingLifecycle {
        self.pipeline.lifecycle()
    }

    pub fn handle_hotkey(&mut self, action: crate::platform::HotkeyAction) {
        match action {
            crate::platform::HotkeyAction::SaveClip => {
                info!("Hotkey: SaveClip");
            }
            crate::platform::HotkeyAction::ToggleRecording => {
                info!("Hotkey: ToggleRecording");
            }
            _ => {}
        }
    }

    pub fn apply_runtime_config(&mut self, new_config: &Config) -> Result<()> {
        info!("Applying runtime configuration changes...");

        if self.config.audio.system_volume != new_config.audio.system_volume {
            info!(
                "Audio: System volume changed from {}% to {}%",
                self.config.audio.system_volume, new_config.audio.system_volume
            );
        }

        if self.config.audio.mic_volume != new_config.audio.mic_volume {
            info!(
                "Audio: Mic volume changed from {}% to {}%",
                self.config.audio.mic_volume, new_config.audio.mic_volume
            );
        }

        if self.config.audio.capture_system != new_config.audio.capture_system {
            warn!(
                "Audio: System capture toggle changed ({} -> {}), requires restart",
                self.config.audio.capture_system, new_config.audio.capture_system
            );
        }

        if self.config.audio.capture_mic != new_config.audio.capture_mic {
            warn!(
                "Audio: Mic capture toggle changed ({} -> {}), requires restart",
                self.config.audio.capture_mic, new_config.audio.capture_mic
            );
        }

        if self.config.general.replay_duration_secs != new_config.general.replay_duration_secs {
            info!(
                "Buffer: Replay duration changed from {}s to {}s (effective on next buffer creation)",
                self.config.general.replay_duration_secs, new_config.general.replay_duration_secs
            );
        }

        self.config = new_config.clone();

        info!("Runtime configuration changes applied successfully");
        Ok(())
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn config_mut(&mut self) -> &mut Config {
        &mut self.config
    }
}
