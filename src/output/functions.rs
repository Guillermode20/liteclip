use crate::encode::EncodedPacket;
use anyhow::{Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Audio sample rate for encoded audio (48 kHz).
#[cfg(feature = "ffmpeg")]
pub const AUDIO_SAMPLE_RATE: u32 = 48_000;
/// Number of audio channels (stereo).
#[cfg(feature = "ffmpeg")]
pub const AUDIO_CHANNELS: u16 = 2;
/// Audio bitrate for AAC encoding.
#[cfg(feature = "ffmpeg")]
pub const AUDIO_BITRATE: &str = "192k";

/// Converts a QPC (QueryPerformanceCounter) delta to aligned PCM byte count.
///
/// Ensures the byte count is aligned to audio frame boundaries for proper
/// encoding.
///
/// # Arguments
///
/// * `delta_qpc` - Time delta in QPC units.
/// * `qpc_freq` - QPC frequency in Hz.
/// * `bytes_per_second` - Bytes per second for the audio stream.
/// * `bytes_per_frame` - Bytes per audio frame.
///
/// # Returns
///
/// Aligned byte count as i64.
#[cfg(feature = "ffmpeg")]
pub fn qpc_delta_to_aligned_pcm_bytes(
    delta_qpc: i64,
    qpc_freq: f64,
    bytes_per_second: f64,
    bytes_per_frame: usize,
) -> i64 {
    if qpc_freq <= 0.0 || bytes_per_second <= 0.0 || bytes_per_frame == 0 {
        return 0;
    }
    let raw_bytes = ((delta_qpc as f64 / qpc_freq) * bytes_per_second).round() as i64;
    let frame_size = bytes_per_frame as i64;
    if raw_bytes >= 0 {
        raw_bytes - (raw_bytes % frame_size)
    } else {
        raw_bytes + ((-raw_bytes) % frame_size)
    }
}

/// Writes silence (zeros) to a file.
///
/// Used for padding audio when there's a gap between streams.
///
/// # Arguments
///
/// * `file` - File to write to.
/// * `byte_count` - Number of silence bytes to write.
///
/// # Errors
///
/// Returns an error if file write fails.
#[cfg(feature = "ffmpeg")]
pub fn write_silence_bytes(file: &mut std::fs::File, mut byte_count: usize) -> Result<()> {
    if byte_count == 0 {
        return Ok(());
    }
    let silence = [0u8; 8192];
    while byte_count > 0 {
        let chunk = byte_count.min(silence.len());
        file.write_all(&silence[..chunk])
            .context("Failed writing PCM silence padding")?;
        byte_count -= chunk;
    }
    Ok(())
}

/// Checks if the data appears to be H.264 format.
///
/// # Arguments
///
/// * `data` - Byte slice to check.
///
/// # Returns
///
/// `true` if the data appears to be H.264 encoded.
pub fn is_h264_format(data: &[u8]) -> bool {
    if data.len() < 4 {
        return false;
    }
    if data.len() >= 4 && data[0] == 0x00 && data[1] == 0x00 && data[2] == 0x00 && data[3] == 0x01 {
        return true;
    }
    if data.len() >= 3 && data[0] == 0x00 && data[1] == 0x00 && data[2] == 0x01 {
        return true;
    }
    matches!(h264_nal_type(data), Some(1..=23))
}

/// Extracts the H.264 NAL unit type from byte data.
///
/// # Arguments
///
/// * `data` - Byte slice containing H.264 NAL unit.
///
/// # Returns
///
/// NAL unit type (0-23 for non-VCL, 24-31 for VCL), or None if parsing fails.
pub fn h264_nal_type(data: &[u8]) -> Option<u8> {
    if data.len() >= 5 && data[0..4] == [0x00, 0x00, 0x00, 0x01] {
        return Some(data[4] & 0x1f);
    }
    if data.len() >= 4 && data[0..3] == [0x00, 0x00, 0x01] {
        return Some(data[3] & 0x1f);
    }
    if data.len() >= 5 {
        let nal_len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
        if nal_len > 0 && data.len() >= 4 + nal_len {
            return Some(data[4] & 0x1f);
        }
    }
    None
}

/// Extracts the HEVC NAL unit type from byte data.
///
/// # Arguments
///
/// * `data` - Byte slice containing HEVC NAL unit.
///
/// # Returns
///
/// NAL unit type, or None if parsing fails.
pub fn hevc_nal_type(data: &[u8]) -> Option<u8> {
    if data.len() >= 6 && data[0..4] == [0x00, 0x00, 0x00, 0x01] {
        return Some((data[4] >> 1) & 0x3f);
    }
    if data.len() >= 5 && data[0..3] == [0x00, 0x00, 0x01] {
        return Some((data[3] >> 1) & 0x3f);
    }
    if data.len() >= 6 {
        let nal_len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
        if nal_len > 1 && data.len() >= 4 + nal_len {
            return Some((data[4] >> 1) & 0x3f);
        }
    }
    None
}

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
pub fn generate_output_filename() -> String {
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
        base_dir.to_path_buf()
    };

    std::fs::create_dir_all(&output_dir)
        .with_context(|| format!("Failed to create output directory: {:?}", output_dir))?;

    Ok(output_dir.join(&filename))
}

/// Generates a thumbnail for a video file using FFmpeg.
///
/// The thumbnail is saved to `<save_directory>/.cache/<hash>.jpg` where the hash
/// is computed from the video path, matching the gallery's thumbnail lookup scheme.
pub fn generate_thumbnail(video_path: &Path, save_directory: &Path) -> Result<PathBuf> {
    use std::hash::{Hash, Hasher};
    use std::process::Command;

    // Compute the cache directory path (same as gallery)
    let cache_dir = save_directory.join(".cache");

    // Create cache directory if it doesn't exist
    std::fs::create_dir_all(&cache_dir)
        .with_context(|| format!("Failed to create cache directory: {:?}", cache_dir))?;

    // Hash the video path to get the thumbnail filename (same as gallery)
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    video_path.hash(&mut hasher);
    let hash = hasher.finish();
    let thumb_path = cache_dir.join(format!("{:016x}.jpg", hash));

    // Skip if thumbnail already exists
    if thumb_path.exists() {
        debug!("Thumbnail already exists: {:?}", thumb_path);
        return Ok(thumb_path);
    }

    debug!(
        "Generating thumbnail for {:?} -> {:?}",
        video_path, thumb_path
    );

    // Use FFmpeg to extract a frame at 1 second into the video
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;

        let status = Command::new("ffmpeg")
            .args([
                "-y",
                "-i",
                &video_path.to_string_lossy(),
                "-ss",
                "00:00:01",
                "-vframes",
                "1",
                "-vf",
                "scale=320:-1",
                "-q:v",
                "5",
                &thumb_path.to_string_lossy(),
            ])
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .context("Failed to spawn ffmpeg for thumbnail generation")?;

        if status.success() {
            info!("Generated thumbnail: {:?}", thumb_path);
        } else {
            warn!(
                "FFmpeg thumbnail extraction failed with status: {:?}",
                status
            );
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let status = Command::new("ffmpeg")
            .args([
                "-y",
                "-i",
                &video_path.to_string_lossy(),
                "-ss",
                "00:00:01",
                "-vframes",
                "1",
                "-vf",
                "scale=320:-1",
                "-q:v",
                "5",
                &thumb_path.to_string_lossy(),
            ])
            .status()
            .context("Failed to spawn ffmpeg for thumbnail generation")?;

        if status.success() {
            info!("Generated thumbnail: {:?}", thumb_path);
        } else {
            warn!(
                "FFmpeg thumbnail extraction failed with status: {:?}",
                status
            );
        }
    }

    Ok(thumb_path)
}

/// Legacy function kept for API compatibility. Use `generate_thumbnail` instead.
pub fn extract_thumbnail(_packet: &EncodedPacket, output_path: &Path) -> Result<PathBuf> {
    debug!("extract_thumbnail called - attempting to derive save directory from output path");

    // Try to derive the save directory from the output path
    // Videos can be in: save_dir/game_name/video.mp4 or save_dir/video.mp4
    // We need to find the save directory (which contains .cache)
    let save_dir = output_path
        .parent()
        .context("Output path has no parent directory")?;

    // Check if parent contains .cache (video is directly in save_dir)
    // or if grandparent contains .cache (video is in game subdirectory)
    let cache_check = save_dir.join(".cache");
    let actual_save_dir = if cache_check.exists() || !save_dir.join(".cache").exists() {
        // Check grandparent
        if let Some(grandparent) = save_dir.parent() {
            if grandparent.join(".cache").exists() {
                grandparent.to_path_buf()
            } else {
                // Assume save_dir is correct (will create .cache there)
                save_dir.to_path_buf()
            }
        } else {
            save_dir.to_path_buf()
        }
    } else {
        save_dir.to_path_buf()
    };

    generate_thumbnail(output_path, &actual_save_dir)
}
