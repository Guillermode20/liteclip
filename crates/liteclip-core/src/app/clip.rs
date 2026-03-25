use crate::{
    buffer::ReplayBuffer,
    config::Config,
    host::CoreHost,
    output::{spawn_clip_saver, MuxerConfig},
};
use anyhow::{bail, Result};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

/// Manages clip saving operations.
///
/// Handles the process of saving the replay buffer to an MP4 file,
/// including output path generation and muxer configuration.
pub struct ClipManager;

impl ClipManager {
    /// Saves the current replay buffer to an MP4 file.
    ///
    /// This is the main entry point for clip saving. It:
    /// 1. Generates an output path with timestamp
    /// 2. Validates buffer has packets and keyframes
    /// 3. Spawns a background task for muxing
    /// 4. Waits for completion and returns the final path
    ///
    /// # Arguments
    ///
    /// * `config` - Application configuration.
    /// * `buffer` - The replay buffer to save.
    /// * `game_name` - Optional game name for folder organization.
    /// * `host` - Optional [`CoreHost`] invoked after a successful save.
    ///
    /// # Returns
    ///
    /// Path to the saved MP4 file.
    ///
    /// # Errors
    ///
    /// - Returns error if buffer is empty
    /// - Returns error if no keyframe is available
    /// - Returns error if muxing fails
    ///
    /// # Example
    ///
    /// ```no_run
    /// use liteclip_core::app::ClipManager;
    /// use liteclip_core::config::Config;
    /// use liteclip_core::buffer::ReplayBuffer;
    ///
    /// // let path = ClipManager::save_clip(&config, &buffer, Some("Valorant"), None).await.unwrap();
    /// ```
    pub async fn save_clip(
        config: &Config,
        buffer: &ReplayBuffer,
        game_name: Option<&str>,
        host: Option<Arc<dyn CoreHost>>,
    ) -> Result<PathBuf> {
        crate::output::saver::log_save_memory("save_clip_entry", Some(buffer), None);
        info!("Clip: saving replay buffer");

        let output_path = Self::generate_output_path(config, game_name)?;

        let stats = buffer.stats();
        info!(
            "Buffer stats before save: {} packets, {} bytes, {} keyframes",
            stats.packet_count, stats.total_bytes, stats.keyframe_count
        );

        if stats.packet_count == 0 {
            warn!("Buffer is empty - cannot save clip");
            bail!("Buffer is empty - no frames to save");
        }

        if stats.keyframe_count == 0 {
            warn!("No keyframe in buffer - cannot save clip yet");
            bail!(
                "No keyframe available - please wait a moment for the next keyframe before saving"
            );
        }

        let (width, height) =
            buffer
                .snapshot_first_packet_resolution()
                .unwrap_or(match config.video.resolution {
                    crate::config::Resolution::Native => (1920, 1080),
                    crate::config::Resolution::P1080 => (1920, 1080),
                    crate::config::Resolution::P720 => (1280, 720),
                    crate::config::Resolution::P480 => (854, 480),
                });
        let fps = config.video.framerate as f64;

        let muxer_config = MuxerConfig::new(width, height, fps, &output_path)
            .with_video_codec("hevc")
            .with_expect_audio(config.audio.capture_system || config.audio.capture_mic);

        let buffer_clone = buffer.clone();
        let duration = Duration::from_secs(config.general.replay_duration_secs as u64);
        let save_directory = PathBuf::from(&config.general.save_directory);

        crate::output::saver::log_save_memory("before_spawn_saver", Some(buffer), None);
        let handle = spawn_clip_saver(
            buffer_clone,
            duration,
            output_path.clone(),
            muxer_config,
            save_directory.clone(),
            config.general.generate_clip_thumbnail,
        );
        let result = handle.await?;
        let final_path = result?;

        info!("Clip saver completed; restarting replay buffer");
        // Drop the existing replay contents and restart so subsequent clips
        // start from a fresh buffer.
        buffer.restart();
        info!("Replay buffer restarted");

        if let Some(h) = host {
            h.on_clip_saved(&final_path);
        }

        Ok(final_path)
    }

    fn generate_output_path(config: &Config, game_name: Option<&str>) -> Result<PathBuf> {
        use chrono::Local;

        let timestamp = Local::now();
        let filename = format!("{}.mp4", timestamp.format("%Y-%m-%d_%H-%M-%S_%3f"));

        let save_dir = PathBuf::from(&config.general.save_directory);

        let output_dir = if let Some(game) = game_name {
            if game.is_empty() || !config.general.auto_detect_game {
                save_dir
            } else {
                save_dir.join(game)
            }
        } else {
            // Put recordings without an associated game into a Desktop subfolder by default
            save_dir.join("Desktop")
        };

        std::fs::create_dir_all(&output_dir)?;

        Ok(output_dir.join(filename))
    }
}
