use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Audio sample rate for encoded audio (48 kHz).
#[cfg(feature = "ffmpeg")]
pub const AUDIO_SAMPLE_RATE: u32 = 48_000;
/// Number of audio channels (stereo).
#[cfg(feature = "ffmpeg")]
pub const AUDIO_CHANNELS: u16 = 2;

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

pub fn ffmpeg_executable_path() -> PathBuf {
    let mut candidates = Vec::new();

    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(exe_dir) = current_exe.parent() {
            candidates.push(exe_dir.join("ffmpeg.exe"));
            if let Some(workspace_root) = exe_dir.parent().and_then(|p| p.parent()) {
                candidates.push(
                    workspace_root
                        .join("ffmpeg_dev")
                        .join("sdk")
                        .join("bin")
                        .join("ffmpeg.exe"),
                );
            }
        }
    }

    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("ffmpeg_dev")
            .join("sdk")
            .join("bin")
            .join("ffmpeg.exe"),
    );

    candidates
        .into_iter()
        .find(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from("ffmpeg"))
}

pub fn remux_fragmented_mp4(input_path: &Path, output_path: &Path, faststart: bool) -> Result<()> {
    use std::process::Command;

    let ffmpeg = ffmpeg_executable_path();
    let mut args = vec![
        "-y".to_string(),
        "-i".to_string(),
        input_path.to_string_lossy().into_owned(),
        "-map".to_string(),
        "0".to_string(),
        "-c".to_string(),
        "copy".to_string(),
    ];

    if faststart {
        args.push("-movflags".to_string());
        args.push("+faststart".to_string());
    }

    args.push(output_path.to_string_lossy().into_owned());

    debug!(
        "Remuxing fragmented MP4 {:?} -> {:?} via {:?}",
        input_path, output_path, ffmpeg
    );

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;

        let status = Command::new(&ffmpeg)
            .args(args.iter().map(|s| s.as_str()))
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .with_context(|| format!("Failed to spawn ffmpeg remux process via {:?}", ffmpeg))?;

        if status.success() {
            info!("Remuxed fragmented MP4 to regular MP4: {:?}", output_path);
            return Ok(());
        }

        anyhow::bail!("FFmpeg remux failed with status {:?}", status);
    }

    #[cfg(not(target_os = "windows"))]
    {
        let status = Command::new(&ffmpeg)
            .args(args.iter().map(|s| s.as_str()))
            .status()
            .with_context(|| format!("Failed to spawn ffmpeg remux process via {:?}", ffmpeg))?;

        if status.success() {
            info!("Remuxed fragmented MP4 to regular MP4: {:?}", output_path);
            return Ok(());
        }

        anyhow::bail!("FFmpeg remux failed with status {:?}", status);
    }
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

    let ffmpeg = ffmpeg_executable_path();

    // Use FFmpeg to extract a frame at 1 second into the video
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;

        let status = Command::new(&ffmpeg)
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
            .with_context(|| {
                format!(
                    "Failed to spawn ffmpeg for thumbnail generation via {:?}",
                    ffmpeg
                )
            })?;

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
        let status = Command::new(&ffmpeg)
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
            .with_context(|| {
                format!(
                    "Failed to spawn ffmpeg for thumbnail generation via {:?}",
                    ffmpeg
                )
            })?;

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
