use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tracing::debug;

/// Audio sample rate for encoded audio (48 kHz).
#[cfg(feature = "ffmpeg")]
pub const AUDIO_SAMPLE_RATE: u32 = 48_000;
/// Number of audio channels (stereo).
#[cfg(feature = "ffmpeg")]
pub const AUDIO_CHANNELS: u16 = 2;

// Re-export NAL type helpers from the shared module.
pub use crate::media::nal::{h264_nal_type, hevc_nal_type};

/// Calculates the starting PTS for a clip based on duration.
///
/// Determines where in the buffer to start when saving a clip of a given duration.
///
/// # Arguments
///
/// * `newest_pts` - The newest PTS in the buffer.
/// * `duration` - Desired clip duration.
/// * `oldest_pts` - Optional oldest PTS in the buffer.
///
/// # Returns
///
/// The starting PTS for the clip.
pub fn calculate_clip_start_pts(
    newest_pts: i64,
    duration: std::time::Duration,
    oldest_pts: Option<i64>,
) -> i64 {
    let qpc_freq = crate::buffer::ring::functions::qpc_frequency();
    let duration_qpc = (duration.as_secs_f64() * qpc_freq as f64) as i64;

    let available_duration_qpc = if let Some(oldest) = oldest_pts {
        newest_pts.saturating_sub(oldest)
    } else {
        duration_qpc
    };

    let has_full_duration = available_duration_qpc >= duration_qpc;

    let start_pts = if has_full_duration {
        let skip_qpc = qpc_freq;
        (newest_pts - duration_qpc + skip_qpc).max(skip_qpc)
    } else {
        newest_pts.saturating_sub(available_duration_qpc).max(0)
    };

    let start_pts = start_pts.max(0);

    debug!(
        "Clip window: newest_pts={}, requested_duration={}s, available_duration={}s, has_full={}, start_pts={}",
        newest_pts,
        duration.as_secs(),
        available_duration_qpc / qpc_freq,
        has_full_duration,
        start_pts
    );
    start_pts
}

/// Generates a timestamped output filename.
///
/// # Returns
///
/// Filename string in format "YYYY-MM-DD_HH-MM-SSS.mp4".
fn generate_output_filename() -> String {
    let timestamp = chrono::Local::now();
    format!("{}.mp4", timestamp.format("%Y-%m-%d_%H-%M-%S_%3f"))
}

/// Generates an output path with optional game subdirectory.
///
/// Creates the output directory if it doesn't exist.
///
/// # Arguments
///
/// * `base_dir` - Base save directory.
/// * `game_name` - Optional game name for subdirectory organization.
///
/// # Returns
///
/// Complete path to the output file.
pub fn generate_output_path(base_dir: &Path, game_name: Option<&str>) -> Result<PathBuf> {
    let filename = generate_output_filename();

    let output_dir = if let Some(game) = game_name {
        if game.is_empty() {
            base_dir.to_path_buf()
        } else {
            base_dir.join(game)
        }
    } else {
        // Default to a Desktop subfolder for recordings that are not associated with a game
        base_dir.join("Desktop")
    };

    std::fs::create_dir_all(&output_dir)
        .with_context(|| format!("Failed to create output directory: {:?}", output_dir))?;

    Ok(output_dir.join(&filename))
}

/// Resolved FFmpeg executable path (see [`crate::runtime`] for search order and overrides).
pub fn ffmpeg_executable_path() -> PathBuf {
    crate::runtime::resolve_ffmpeg_executable()
}

pub fn remux_fragmented_mp4(input_path: &Path, output_path: &Path, faststart: bool) -> Result<()> {
    crate::output::sdk_ffmpeg_output::remux_fragmented_mp4(input_path, output_path, faststart)
}

/// Generates a thumbnail for a video file using FFmpeg.
///
/// The thumbnail is saved to `<save_directory>/.cache/<hash>.jpg` where the hash
/// is computed from the video path, matching the gallery's thumbnail lookup scheme.
pub fn generate_thumbnail(video_path: &Path, save_directory: &Path) -> Result<PathBuf> {
    crate::output::sdk_ffmpeg_output::generate_thumbnail(video_path, save_directory)
}

#[cfg(test)]
mod tests {
    use super::*;

    // NAL type tests live in crate::media::nal::tests

    #[test]
    fn generate_output_path_with_game() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = generate_output_path(temp.path(), Some("MyGame")).unwrap();
        assert!(path.to_string_lossy().contains("MyGame"));
        assert!(path.parent().unwrap().exists());
    }

    #[test]
    fn generate_output_path_without_game() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = generate_output_path(temp.path(), None).unwrap();
        assert!(path.to_string_lossy().contains("Desktop"));
        assert!(path.parent().unwrap().exists());
    }

    #[test]
    fn generate_output_path_empty_game() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = generate_output_path(temp.path(), Some("")).unwrap();
        assert_eq!(path.parent().unwrap(), temp.path());
    }
}
