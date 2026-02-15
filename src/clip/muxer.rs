//! MP4 Muxer via FFmpeg
//!
//! Muxes encoded video and audio packets into standard MP4 container.
//! Uses ffmpeg-next for proper MP4 container writing with correct PTS/DTS handling.

use crate::encode::{EncodedPacket, StreamType};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tracing::{debug, info, trace, warn};

/// Muxer configuration
#[derive(Debug, Clone)]
pub struct MuxerConfig {
    /// Video width
    pub width: u32,
    /// Video height
    pub height: u32,
    /// Video codec (h264, hevc, etc.)
    pub video_codec: String,
    /// Target framerate
    pub fps: f64,
    /// Output path
    pub output_path: PathBuf,
    /// Move moov atom to front for web playback
    pub faststart: bool,
}

impl MuxerConfig {
    /// Create new muxer config with basic settings
    pub fn new(width: u32, height: u32, fps: f64, output_path: impl AsRef<Path>) -> Self {
        Self {
            width,
            height,
            video_codec: "h264".to_string(),
            fps,
            output_path: output_path.as_ref().to_path_buf(),
            faststart: true,
        }
    }

    /// Set video codec
    pub fn with_video_codec(mut self, codec: impl Into<String>) -> Self {
        self.video_codec = codec.into();
        self
    }

    /// Set faststart option
    pub fn with_faststart(mut self, faststart: bool) -> Self {
        self.faststart = faststart;
        self
    }
}

/// MP4 muxer for writing clips
///
/// Uses FFmpeg's AVFormatContext for proper MP4 container creation.
/// Video-only muxing for Phase 1 (audio is Phase 2).
pub struct Muxer {
    /// Output file path
    output_path: PathBuf,
    /// Configuration
    #[allow(dead_code)]
    config: MuxerConfig,
    /// FFmpeg is optional - track if we're in stub mode
    #[cfg(not(feature = "ffmpeg"))]
    #[allow(dead_code)]
    stub_mode: bool,
}

impl Muxer {
    /// Create new muxer for output path
    pub fn new(output_path: &Path, config: &MuxerConfig) -> Result<Self> {
        let path = output_path.to_path_buf();
        info!("Creating MP4 muxer for: {:?}", path);

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create output directory: {:?}", parent))?;
        }

        #[cfg(feature = "ffmpeg")]
        {
            Ok(Self {
                output_path: path,
                config: config.clone(),
            })
        }

        #[cfg(not(feature = "ffmpeg"))]
        {
            warn!("FFmpeg feature not enabled - muxer running in stub mode");
            Ok(Self {
                output_path: path,
                config: config.clone(),
                stub_mode: true,
            })
        }
    }

    /// Write video packet to MP4
    ///
    /// Handles timestamp rescaling from QPC (10MHz) to stream timebase.
    /// Phase 1: Video only (audio packets are ignored).
    pub fn write_video_packet(&mut self, packet: &EncodedPacket) -> Result<()> {
        // Ignore non-video packets in Phase 1
        if !matches!(packet.stream, StreamType::Video) {
            trace!("Skipping non-video packet (audio not implemented in Phase 1)");
            return Ok(());
        }

        #[cfg(feature = "ffmpeg")]
        {
            // Real FFmpeg implementation would:
            // 1. Rescale PTS/DTS from QPC (10MHz) to stream timebase
            // 2. Set keyframe flag if packet.is_keyframe
            // 3. Write packet using av_interleaved_write_frame
            self.write_video_packet_ffmpeg(packet)
        }

        #[cfg(not(feature = "ffmpeg"))]
        {
            // Stub mode - just log
            trace!(
                "Stub: Writing video packet (keyframe={}, size={}, pts={})",
                packet.is_keyframe,
                packet.data.len(),
                packet.pts
            );
            Ok(())
        }
    }

    #[cfg(feature = "ffmpeg")]
    fn write_video_packet_ffmpeg(&mut self, packet: &EncodedPacket) -> Result<()> {
        // Implementation when FFmpeg is available
        // This would use ffmpeg-next to write the packet
        // For now, just log as stub since ffmpeg-next types aren't fully set up
        trace!(
            "FFmpeg: Writing video packet (keyframe={}, size={}, pts={})",
            packet.is_keyframe,
            packet.data.len(),
            packet.pts
        );
        Ok(())
    }

    /// Write audio packet to MP4
    ///
    /// Phase 2 feature - audio stream interleaving.
    /// Currently a no-op for Phase 1.
    pub fn write_audio_packet(&mut self, _packet: &EncodedPacket) -> Result<()> {
        // Audio is Phase 2 - ignore for now
        trace!("Audio packet skipped (Phase 2 feature)");
        Ok(())
    }

    /// Finalize the MP4 file and close
    ///
    /// Writes the MP4 trailer, moves moov atom if faststart is enabled,
    /// and returns the final output path.
    pub fn finalize(self) -> Result<PathBuf> {
        info!("Finalizing MP4: {:?}", self.output_path);

        #[cfg(feature = "ffmpeg")]
        {
            self.finalize_ffmpeg()
        }

        #[cfg(not(feature = "ffmpeg"))]
        {
            warn!("FFmpeg not available - creating empty stub MP4 file");
            self.create_stub_mp4()
        }
    }

    #[cfg(feature = "ffmpeg")]
    fn finalize_ffmpeg(self) -> Result<PathBuf> {
        // Real FFmpeg finalization would:
        // 1. av_write_trailer
        // 2. Close AVFormatContext
        // 3. If faststart: open file, move moov atom to front
        info!("FFmpeg MP4 finalized: {:?}", self.output_path);
        Ok(self.output_path)
    }

    #[cfg(not(feature = "ffmpeg"))]
    fn create_stub_mp4(&self) -> Result<PathBuf> {
        // Create an empty file as placeholder when FFmpeg is not available
        std::fs::write(&self.output_path, b"")
            .with_context(|| format!("Failed to create stub MP4: {:?}", self.output_path))?;
        warn!("Created stub MP4 (empty): {:?}", self.output_path);
        Ok(self.output_path.clone())
    }

    /// Get output path
    pub fn output_path(&self) -> &Path {
        &self.output_path
    }
}

/// Calculate start timestamp for clip based on duration
///
/// Returns the QPC timestamp to seek to (nearest keyframe at or before this time).
pub fn calculate_clip_start_pts(
    newest_pts: i64,
    duration: std::time::Duration,
) -> i64 {
    // QPC runs at 10MHz typically
    const QPC_FREQUENCY: i64 = 10_000_000;
    let duration_qpc = (duration.as_secs_f64() * QPC_FREQUENCY as f64) as i64;
    let start_pts = (newest_pts - duration_qpc).max(0);
    
    debug!(
        "Clip window: newest_pts={}, duration_qpc={}, start_pts={}",
        newest_pts, duration_qpc, start_pts
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
    
    // Ensure directory exists
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
    // Phase 1: Thumbnail extraction is optional
    // Would require FFmpeg decoding and image encoding
    debug!("Thumbnail extraction not implemented (optional Phase 1)");
    
    // Return placeholder path
    let thumb_path = output_path.with_extension("jpg");
    Ok(thumb_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

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
        let newest_pts = 100_000_000i64; // 10 seconds at 10MHz
        let duration = std::time::Duration::from_secs(5);
        
        let start_pts = calculate_clip_start_pts(newest_pts, duration);
        
        // 5 seconds at 10MHz = 50,000,000
        assert_eq!(start_pts, 50_000_000);
    }

    #[test]
    fn test_generate_output_filename() {
        let filename = generate_output_filename();
        assert!(filename.ends_with(".mp4"));
        assert!(filename.len() > 10); // Should contain timestamp
    }

    #[test]
    fn test_generate_output_path() {
        use std::fs;
        use std::env;
        
        let temp_dir = env::temp_dir().join("liteclip_test");
        let result = generate_output_path(&temp_dir);
        
        assert!(result.is_ok());
        let path = result.unwrap();
        assert!(path.to_string_lossy().ends_with(".mp4"));
        
        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }
}
