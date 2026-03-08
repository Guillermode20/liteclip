//! Auto-generated module
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use crate::encode::EncodedPacket;
use anyhow::{Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing::debug;

#[cfg(feature = "ffmpeg")]
pub const AUDIO_SAMPLE_RATE: u32 = 48_000;
#[cfg(feature = "ffmpeg")]
pub const AUDIO_CHANNELS: u16 = 2;
#[cfg(feature = "ffmpeg")]
pub const AUDIO_BITRATE: &str = "192k";
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
/// Detect if packet data is H.264 format by looking for NAL start codes
///
/// H.264 NAL units start with either:
/// - 0x00 0x00 0x00 0x01 (4-byte start code)
/// - 0x00 0x00 0x01 (3-byte start code)
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
/// Calculate start timestamp for clip based on duration
///
/// Returns the QPC timestamp to seek to (nearest keyframe at or before this time).
///
/// Smart skipping logic:
/// - If buffer has enough data for requested duration, skip first 1 second to avoid
///   potential encoder initialization artifacts
/// - If buffer has less data than requested, don't skip - use all available data
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
/// Generate output filename with timestamp
///
/// Format: {timestamp}.mp4 (e.g., 2026-02-15_20-03-05.mp4)
/// Phase 1: Simple timestamp filenames
/// Phase 3: Will include game name in path
pub fn generate_output_filename() -> String {
    let timestamp = chrono::Local::now();
    format!("{}.mp4", timestamp.format("%Y-%m-%d_%H-%M-%S_%3f"))
}
/// Generate full output path for clip
///
/// Creates directory structure if needed.
/// Phase 1: Saves to base directory with timestamp filename.
pub fn generate_output_path(base_dir: &Path) -> Result<PathBuf> {
    let filename = generate_output_filename();
    let output_path = base_dir.join(&filename);
    std::fs::create_dir_all(base_dir)
        .with_context(|| format!("Failed to create output directory: {:?}", base_dir))?;
    Ok(output_path)
}
/// Extract thumbnail from keyframe
///
/// Optional Phase 1 feature: Decodes first keyframe to RGB, encodes as JPEG.
/// Returns path to thumbnail file.
pub fn extract_thumbnail(_packet: &EncodedPacket, output_path: &Path) -> Result<PathBuf> {
    debug!("Thumbnail extraction not implemented (optional Phase 1)");
    let thumb_path = output_path.with_extension("jpg");
    Ok(thumb_path)
}
#[cfg(test)]
mod tests {
    use super::super::types::MuxerConfig;
    use super::*;
    #[test]
    fn test_muxer_config_creation() {
        let config = MuxerConfig::new(1920, 1080, 30.0, "/tmp/test.mp4")
            .with_video_codec("h264")
            .with_faststart(true);
        assert_eq!(config.width, 1920);
        assert_eq!(config.height, 1080);
        assert_eq!(config.fps, 30.0);
        assert_eq!(config.video_codec, "h264");
        assert!(config.faststart);
    }
    #[test]
    fn test_calculate_clip_start_pts() {
        let newest_pts = 100_000_000i64;
        let duration = std::time::Duration::from_secs(5);
        let start_pts = calculate_clip_start_pts(newest_pts, duration);
        assert_eq!(start_pts, 60_000_000); // 100_000_000 - 50_000_000 + 10_000_000 (1s skip)
    }
    #[test]
    fn test_generate_output_filename() {
        let filename = generate_output_filename();
        assert!(filename.ends_with(".mp4"));
        assert!(filename.len() > 10);
    }
    #[test]
    fn test_generate_output_path() {
        use std::env;
        use std::fs;
        let temp_dir = env::temp_dir().join("liteclip_test");
        let result = generate_output_path(&temp_dir);
        assert!(result.is_ok());
        let path = result.unwrap();
        assert!(path.to_string_lossy().ends_with(".mp4"));
        let _ = fs::remove_dir_all(&temp_dir);
    }
    #[test]
    fn test_is_h264_format_4byte_start_code() {
        let h264_data = vec![0x00, 0x00, 0x00, 0x01, 0x67, 0x64, 0x00, 0x1f];
        assert!(is_h264_format(&h264_data));
    }
    #[test]
    fn test_is_h264_format_3byte_start_code() {
        let h264_data = vec![0x00, 0x00, 0x01, 0x67, 0x64, 0x00, 0x1f];
        assert!(is_h264_format(&h264_data));
    }
    #[test]
    fn test_is_h264_format_length_prefixed() {
        let h264_data = vec![0x00, 0x00, 0x00, 0x04, 0x67, 0x64, 0x00, 0x1f];
        assert!(is_h264_format(&h264_data));
    }
    #[test]
    fn test_is_h264_format_mjpeg() {
        let mjpeg_data = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46];
        assert!(!is_h264_format(&mjpeg_data));
    }
    #[test]
    fn test_is_h264_format_too_short() {
        let short_data = vec![0x00, 0x00];
        assert!(!is_h264_format(&short_data));
    }
    #[test]
    fn test_hevc_nal_type_parsing() {
        // 4-byte start code + HEVC NAL header where nal_unit_type = 32 (VPS)
        let hevc_data = vec![0x00, 0x00, 0x00, 0x01, 0x40, 0x01];
        assert_eq!(hevc_nal_type(&hevc_data), Some(32));
    }
    #[test]
    fn test_h264_nal_type_length_prefixed() {
        let h264_data = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x21];
        assert_eq!(h264_nal_type(&h264_data), Some(5));
    }
    #[test]
    fn test_hevc_nal_type_length_prefixed() {
        let hevc_data = vec![0x00, 0x00, 0x00, 0x04, 0x40, 0x01, 0x0c, 0x01];
        assert_eq!(hevc_nal_type(&hevc_data), Some(32));
    }
    #[cfg(feature = "ffmpeg")]
    #[test]
    fn test_qpc_delta_to_aligned_pcm_bytes_positive() {
        let bytes = qpc_delta_to_aligned_pcm_bytes(5_000_000, 10_000_000.0, 192_000.0, 4);
        assert_eq!(bytes, 96_000);
    }
    #[cfg(feature = "ffmpeg")]
    #[test]
    fn test_qpc_delta_to_aligned_pcm_bytes_negative_is_frame_aligned() {
        let bytes = qpc_delta_to_aligned_pcm_bytes(-2_500_000, 10_000_000.0, 192_000.0, 4);
        assert_eq!(bytes, -48_000);
        assert_eq!(bytes % 4, 0);
    }
}
