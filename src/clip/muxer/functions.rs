//! Auto-generated module
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use crate::encode::EncodedPacket;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::io::Write;
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
        file.write_all(&silence[..chunk]).context("Failed writing PCM silence padding")?;
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
    if data.len() >= 4 && data[0] == 0x00 && data[1] == 0x00 && data[2] == 0x00
        && data[3] == 0x01
    {
        return true;
    }
    if data.len() >= 3 && data[0] == 0x00 && data[1] == 0x00 && data[2] == 0x01 {
        return true;
    }
    false
}
pub fn h264_nal_type(data: &[u8]) -> Option<u8> {
    if data.len() >= 5 && data[0..4] == [0x00, 0x00, 0x00, 0x01] {
        return Some(data[4] & 0x1f);
    }
    if data.len() >= 4 && data[0..3] == [0x00, 0x00, 0x01] {
        return Some(data[3] & 0x1f);
    }
    None
}
/// Calculate start timestamp for clip based on duration
///
/// Returns the QPC timestamp to seek to (nearest keyframe at or before this time).
pub fn calculate_clip_start_pts(newest_pts: i64, duration: std::time::Duration) -> i64 {
    let mut qpc_freq = 10_000_000i64;
    unsafe {
        let _ = windows::Win32::System::Performance::QueryPerformanceFrequency(
            &mut qpc_freq,
        );
    }
    let duration_qpc = (duration.as_secs_f64() * qpc_freq as f64) as i64;
    let start_pts = (newest_pts - duration_qpc).max(0);
    debug!(
        "Clip window: newest_pts={}, duration_qpc={}, start_pts={}", newest_pts,
        duration_qpc, start_pts
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
    format!("{}.mp4", timestamp.format("%Y-%m-%d_%H-%M-%S"))
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
pub fn extract_thumbnail(
    _packet: &EncodedPacket,
    output_path: &Path,
) -> Result<PathBuf> {
    debug!("Thumbnail extraction not implemented (optional Phase 1)");
    let thumb_path = output_path.with_extension("jpg");
    Ok(thumb_path)
}
#[cfg(test)]
mod tests {
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
        assert_eq!(start_pts, 50_000_000);
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
        assert!(is_h264_format(& h264_data));
    }
    #[test]
    fn test_is_h264_format_3byte_start_code() {
        let h264_data = vec![0x00, 0x00, 0x01, 0x67, 0x64, 0x00, 0x1f];
        assert!(is_h264_format(& h264_data));
    }
    #[test]
    fn test_is_h264_format_mjpeg() {
        let mjpeg_data = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46];
        assert!(! is_h264_format(& mjpeg_data));
    }
    #[test]
    fn test_is_h264_format_too_short() {
        let short_data = vec![0x00, 0x00];
        assert!(! is_h264_format(& short_data));
    }
    #[cfg(feature = "ffmpeg")]
    #[test]
    fn test_qpc_delta_to_aligned_pcm_bytes_positive() {
        let bytes = qpc_delta_to_aligned_pcm_bytes(
            5_000_000,
            10_000_000.0,
            192_000.0,
            4,
        );
        assert_eq!(bytes, 96_000);
    }
    #[cfg(feature = "ffmpeg")]
    #[test]
    fn test_qpc_delta_to_aligned_pcm_bytes_negative_is_frame_aligned() {
        let bytes = qpc_delta_to_aligned_pcm_bytes(
            -2_500_000,
            10_000_000.0,
            192_000.0,
            4,
        );
        assert_eq!(bytes, - 48_000);
        assert_eq!(bytes % 4, 0);
    }
    #[cfg(feature = "ffmpeg")]
    #[test]
    fn test_write_audio_temp_pcm_tracks_dual_stream_outputs_two_tracks() {
        use std::env;
        use std::fs;
        let temp_dir = env::temp_dir().join("liteclip_muxer_dual_track_test");
        let _ = fs::create_dir_all(&temp_dir);
        let output = temp_dir.join("out.mp4");
        let config = MuxerConfig::new(1280, 720, 60.0, &output);
        let mut muxer = Muxer::new(&output, &config).expect("muxer init");
        muxer
            .video_packets
            .push(
                EncodedPacket::new(
                    vec![0x00, 0x00, 0x00, 0x01, 0x67],
                    10_000_000,
                    10_000_000,
                    true,
                    StreamType::Video,
                ),
            );
        muxer
            .audio_packets
            .push(
                EncodedPacket::new(
                    vec![0, 1, 2, 3],
                    10_000_000,
                    10_000_000,
                    false,
                    StreamType::SystemAudio,
                ),
            );
        muxer
            .audio_packets
            .push(
                EncodedPacket::new(
                    vec![4, 5, 6, 7],
                    10_000_000,
                    10_000_000,
                    false,
                    StreamType::Microphone,
                ),
            );
        let tracks = muxer.write_audio_temp_pcm_tracks().expect("build audio tracks");
        assert_eq!(tracks.len(), 2);
        assert!(tracks.iter().any(| track | track.title == "system"));
        assert!(tracks.iter().any(| track | track.title == "microphone"));
        assert!(tracks.iter().all(| track | track.path.exists()));
        for track in tracks {
            let _ = fs::remove_file(track.path);
        }
        let _ = fs::remove_dir_all(&temp_dir);
    }
    #[cfg(feature = "ffmpeg")]
    #[test]
    fn test_write_audio_temp_pcm_tracks_single_stream_output() {
        use std::env;
        use std::fs;
        let temp_dir = env::temp_dir().join("liteclip_muxer_single_track_test");
        let _ = fs::create_dir_all(&temp_dir);
        let output = temp_dir.join("out.mp4");
        let config = MuxerConfig::new(1280, 720, 60.0, &output);
        let mut muxer = Muxer::new(&output, &config).expect("muxer init");
        muxer
            .video_packets
            .push(
                EncodedPacket::new(
                    vec![0x00, 0x00, 0x00, 0x01, 0x67],
                    10_000_000,
                    10_000_000,
                    true,
                    StreamType::Video,
                ),
            );
        muxer
            .audio_packets
            .push(
                EncodedPacket::new(
                    vec![4, 5, 6, 7],
                    10_000_000,
                    10_000_000,
                    false,
                    StreamType::Microphone,
                ),
            );
        let tracks = muxer.write_audio_temp_pcm_tracks().expect("build audio tracks");
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].title, "microphone");
        assert!(tracks[0].path.exists());
        for track in tracks {
            let _ = fs::remove_file(track.path);
        }
        let _ = fs::remove_dir_all(&temp_dir);
    }
    #[cfg(feature = "ffmpeg")]
    #[test]
    fn test_write_audio_temp_pcm_tracks_silent_fallback_when_expected() {
        use std::env;
        use std::fs;
        let temp_dir = env::temp_dir().join("liteclip_muxer_silent_fallback_test");
        let _ = fs::create_dir_all(&temp_dir);
        let output = temp_dir.join("out.mp4");
        let config = MuxerConfig::new(1280, 720, 60.0, &output).with_expect_audio(true);
        let muxer = Muxer::new(&output, &config).expect("muxer init");
        let tracks = muxer.write_audio_temp_pcm_tracks().expect("build fallback track");
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].title, "system");
        assert!(tracks[0].path.exists());
        for track in tracks {
            let _ = fs::remove_file(track.path);
        }
        let _ = fs::remove_dir_all(&temp_dir);
    }
}
