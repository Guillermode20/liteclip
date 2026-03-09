use crate::{
    buffer::ReplayBuffer,
    config::Config,
    output::{spawn_clip_saver, MuxerConfig},
};
use anyhow::{bail, Result};
use std::path::PathBuf;
use std::time::Duration;
use tracing::{info, warn};

pub struct ClipManager;

impl ClipManager {
    pub async fn save_clip(config: &Config, buffer: &ReplayBuffer, game_name: Option<&str>) -> Result<PathBuf> {
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

        let handle = spawn_clip_saver(buffer_clone, duration, output_path.clone(), muxer_config);
        let result = handle.await?;
        let final_path = result?;

        info!("Clip saver completed (buffer preserved for continuous replay)");

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
            save_dir
        };

        std::fs::create_dir_all(&output_dir)?;

        Ok(output_dir.join(filename))
    }
}
