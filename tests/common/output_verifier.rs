//! Output verification utilities for validating saved clips.
//!
//! Provides functions to verify MP4 file structure, video properties,
/// and audio synchronization.
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// Verifies that an MP4 file has valid structure.
///
/// Uses ffprobe to check:
/// - File is not corrupted
/// - Contains at least one video stream
/// - Container format is valid
///
/// Requires FFmpeg to be installed and available in PATH.
pub fn verify_mp4_structure(path: &Path) -> Result<()> {
    let path_str = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Path contains invalid UTF-8: {:?}", path))?;

    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=format_name",
            "-of",
            "default=noprint_wrappers=1",
            path_str,
        ])
        .output()
        .context("Failed to run ffprobe. Is FFmpeg installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("ffprobe failed: {}", stderr);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.contains("mp4") && !stdout.contains("mov") {
        anyhow::bail!("File does not appear to be a valid MP4: {}", stdout);
    }

    // Verify video stream exists
    let path_str = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Path contains invalid UTF-8: {:?}", path))?;

    let video_output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_type",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            path_str,
        ])
        .output()
        .context("Failed to check video stream")?;

    if !video_output.status.success() {
        anyhow::bail!("No video stream found in file");
    }

    let video_stdout = String::from_utf8_lossy(&video_output.stdout);
    if !video_stdout.trim().contains("video") {
        anyhow::bail!("File does not contain a video stream");
    }

    Ok(())
}

/// Video properties extracted from a clip file.
#[derive(Debug, Clone)]
pub struct VideoProperties {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration_secs: f64,
    pub codec: String,
    pub bitrate_kbps: u32,
}

impl VideoProperties {
    /// Returns resolution as a tuple (width, height).
    pub fn resolution(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

/// Extracts video properties from a clip file using ffprobe.
///
/// Requires FFmpeg to be installed and available in PATH.
pub fn extract_video_properties(path: &Path) -> Result<VideoProperties> {
    let path_str = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Path contains invalid UTF-8: {:?}", path))?;

    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,r_frame_rate,codec_name",
            "-show_entries",
            "format=duration,bit_rate",
            "-of",
            "json",
            path_str,
        ])
        .output()
        .context("Failed to run ffprobe. Is FFmpeg installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("ffprobe failed: {}", stderr);
    }

    // Parse JSON output
    let json_str = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value =
        serde_json::from_str(&json_str).context("Failed to parse ffprobe output")?;

    let stream = json
        .get("streams")
        .and_then(|s| s.as_array())
        .and_then(|arr| arr.first())
        .context("No video stream found")?;

    let format = json.get("format").context("No format info found")?;

    // Parse frame rate (ffprobe returns as "30/1" format)
    let fps_str = stream
        .get("r_frame_rate")
        .and_then(|v| v.as_str())
        .unwrap_or("0/1");
    let fps = parse_fps(fps_str)?;

    // Parse duration
    let duration_secs = format
        .get("duration")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);

    // Parse bitrate
    let bitrate = format
        .get("bit_rate")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    Ok(VideoProperties {
        width: stream.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
        height: stream.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
        fps,
        duration_secs,
        codec: stream
            .get("codec_name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string(),
        bitrate_kbps: (bitrate / 1000) as u32,
    })
}

/// Parses frame rate from ffprobe format (e.g., "30000/1001" or "30/1").
fn parse_fps(fps_str: &str) -> Result<f64> {
    let parts: Vec<&str> = fps_str.split('/').collect();
    if parts.len() == 2 {
        let num = parts[0].parse::<f64>()?;
        let den = parts[1].parse::<f64>()?;
        if den != 0.0 {
            return Ok(num / den);
        }
    }
    Ok(fps_str.parse::<f64>().unwrap_or(0.0))
}

/// Verifies that a clip's properties match expected values.
///
/// Uses approximate matching for floating-point values.
pub fn verify_video_properties(
    path: &Path,
    expected: &VideoProperties,
    tolerance: f64,
) -> Result<()> {
    let actual = extract_video_properties(path)
        .with_context(|| format!("Failed to extract properties from {:?}", path))?;

    // Check resolution
    if actual.width != expected.width || actual.height != expected.height {
        anyhow::bail!(
            "Resolution mismatch: expected {}x{}, got {}x{}",
            expected.width,
            expected.height,
            actual.width,
            actual.height
        );
    }

    // Check FPS
    if (actual.fps - expected.fps).abs() > tolerance {
        anyhow::bail!(
            "FPS mismatch: expected {:.2}, got {:.2}",
            expected.fps,
            actual.fps
        );
    }

    // Check duration
    if (actual.duration_secs - expected.duration_secs).abs() > tolerance {
        anyhow::bail!(
            "Duration mismatch: expected {:.2}s, got {:.2}s",
            expected.duration_secs,
            actual.duration_secs
        );
    }

    // Check codec (case insensitive)
    if !actual.codec.eq_ignore_ascii_case(&expected.codec) {
        anyhow::bail!(
            "Codec mismatch: expected {}, got {}",
            expected.codec,
            actual.codec
        );
    }

    Ok(())
}

/// Verifies that a clip file has a reasonable duration.
///
/// Useful for checking that clips were saved with the expected replay duration.
pub fn verify_clip_duration(path: &Path, expected_secs: f64, tolerance_secs: f64) -> Result<()> {
    let props = extract_video_properties(path)?;

    let diff = (props.duration_secs - expected_secs).abs();
    if diff > tolerance_secs {
        anyhow::bail!(
            "Duration mismatch: expected {:.2}s ±{:.2}s, got {:.2}s (diff: {:.2}s)",
            expected_secs,
            tolerance_secs,
            props.duration_secs,
            diff
        );
    }

    Ok(())
}

/// Verifies that a clip file is not empty and has reasonable size.
///
/// Minimum size check ensures the file was actually written.
pub fn verify_clip_not_empty(path: &Path, min_size_bytes: u64) -> Result<()> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("Failed to read metadata for {:?}", path))?;

    let size = metadata.len();
    if size < min_size_bytes {
        anyhow::bail!(
            "Clip file too small: {} bytes (min expected: {} bytes)",
            size,
            min_size_bytes
        );
    }

    Ok(())
}

/// Assert helper that verifies MP4 structure or panics with details.
pub fn assert_valid_mp4(path: &Path) {
    if let Err(e) = verify_mp4_structure(path) {
        panic!("MP4 validation failed for {:?}: {}", path, e);
    }
}

/// Assert helper that verifies video properties or panics with details.
pub fn assert_video_properties(path: &Path, expected: &VideoProperties, tolerance: f64) {
    if let Err(e) = verify_video_properties(path, expected, tolerance) {
        panic!("Video property validation failed for {:?}: {}", path, e);
    }
}
