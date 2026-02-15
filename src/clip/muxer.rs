//! MP4 Muxer via FFmpeg
//!
//! Muxes encoded video and audio packets into standard MP4 container.
//! Uses optional FFmpeg pipeline integration for MP4 container writing.

use crate::encode::{EncodedPacket, StreamType};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
#[cfg(feature = "ffmpeg")]
use std::{
    ffi::OsString,
    io::Write,
    process::{Command, Stdio},
};
use tracing::{debug, error, info, trace};

/// Detect if packet data is H.264 format by looking for NAL start codes
///
/// H.264 NAL units start with either:
/// - 0x00 0x00 0x00 0x01 (4-byte start code)
/// - 0x00 0x00 0x01 (3-byte start code)
fn is_h264_format(data: &[u8]) -> bool {
    if data.len() < 4 {
        return false;
    }

    // Check for 4-byte start code: 0x00 0x00 0x00 0x01
    if data.len() >= 4 && data[0] == 0x00 && data[1] == 0x00 && data[2] == 0x00 && data[3] == 0x01 {
        return true;
    }

    // Check for 3-byte start code: 0x00 0x00 0x01
    if data.len() >= 3 && data[0] == 0x00 && data[1] == 0x00 && data[2] == 0x01 {
        return true;
    }

    // Also check for AVCC format (length-prefixed) which starts with version/profile/level
    // Common H.264 profiles have these bytes: 0x00 0x00 0x00 (length) followed by NAL unit
    // For now, we focus on start codes which are more common from hardware encoders

    false
}

fn h264_nal_type(data: &[u8]) -> Option<u8> {
    if data.len() >= 5 && data[0..4] == [0x00, 0x00, 0x00, 0x01] {
        return Some(data[4] & 0x1f);
    }

    if data.len() >= 4 && data[0..3] == [0x00, 0x00, 0x01] {
        return Some(data[3] & 0x1f);
    }

    None
}

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
    /// Buffered video packets used for MP4 generation at finalize()
    #[cfg(feature = "ffmpeg")]
    video_packets: Vec<EncodedPacket>,
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
                video_packets: Vec::new(),
            })
        }

        #[cfg(not(feature = "ffmpeg"))]
        {
            tracing::warn!("FFmpeg feature not enabled - muxer running in stub mode");
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
            self.video_packets.push(packet.clone());
            Ok(())
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
            tracing::warn!("FFmpeg feature disabled - cannot produce MP4");
            self.create_stub_mp4()
        }
    }

    #[cfg(feature = "ffmpeg")]
    fn finalize_ffmpeg(mut self) -> Result<PathBuf> {
        let mut qpc_freq = 10_000_000i64;
        unsafe {
            let _ = windows::Win32::System::Performance::QueryPerformanceFrequency(&mut qpc_freq);
        }
        let qpc_frequency_f64 = qpc_freq as f64;
        let ffmpeg_cmd = self.resolve_ffmpeg_command();

        if self.video_packets.is_empty() {
            bail!("No video packets available for MP4 generation");
        }

        self.video_packets.sort_by_key(|packet| packet.pts);

        // Detect if frames are already H.264 encoded
        let is_h264 = self
            .video_packets
            .first()
            .map(|p| is_h264_format(&p.data))
            .unwrap_or(false);

        if is_h264 {
            info!("Detected H.264 format - using fast muxing path (no transcoding)");
            return self.finalize_ffmpeg_h264_copy(ffmpeg_cmd, qpc_frequency_f64);
        }

        // Fall back to MJPEG transcoding path
        info!("Detected MJPEG format - using transcoding path");
        self.finalize_ffmpeg_mjpeg_transcode(ffmpeg_cmd, qpc_frequency_f64)
    }

    /// Fast path: Mux pre-encoded H.264 frames directly to MP4 without transcoding
    ///
    /// Uses FFmpeg's -c:v copy to just remux the H.264 NAL units into MP4 container.
    /// This is orders of magnitude faster than transcoding.
    #[cfg(feature = "ffmpeg")]
    fn finalize_ffmpeg_h264_copy(
        &self,
        ffmpeg_cmd: OsString,
        qpc_frequency_f64: f64,
    ) -> Result<PathBuf> {
        let effective_fps = if self.video_packets.len() >= 2 {
            let first_pts = self.video_packets.first().map(|p| p.pts).unwrap_or(0);
            let last_pts = self
                .video_packets
                .last()
                .map(|p| p.pts)
                .unwrap_or(first_pts);
            let span_qpc = (last_pts - first_pts).max(1) as f64;
            let span_secs = span_qpc / qpc_frequency_f64;
            if span_secs > 0.0 {
                (self.video_packets.len() as f64 / span_secs).clamp(0.1, self.config.fps.max(1.0))
            } else {
                self.config.fps.max(1.0)
            }
        } else {
            self.config.fps.max(1.0)
        };

        info!(
            "Muxing {} H.264 frames with FPS {:.3}",
            self.video_packets.len(),
            effective_fps
        );

        // Write H.264 stream to a temporary file
        let h264_temp_path = self.output_path.with_extension("h264");
        {
            let mut h264_file = std::fs::File::create(&h264_temp_path).with_context(|| {
                format!("Failed to create temp H.264 file: {:?}", h264_temp_path)
            })?;

            let mut first_idr_index: Option<usize> = None;
            let mut has_sps_before_idr = false;
            let mut has_pps_before_idr = false;
            let mut first_sps: Option<&[u8]> = None;
            let mut first_pps: Option<&[u8]> = None;

            for (index, packet) in self.video_packets.iter().enumerate() {
                match h264_nal_type(packet.data.as_ref()) {
                    Some(7) => {
                        if first_sps.is_none() {
                            first_sps = Some(packet.data.as_ref());
                        }
                        if first_idr_index.is_none() {
                            has_sps_before_idr = true;
                        }
                    }
                    Some(8) => {
                        if first_pps.is_none() {
                            first_pps = Some(packet.data.as_ref());
                        }
                        if first_idr_index.is_none() {
                            has_pps_before_idr = true;
                        }
                    }
                    Some(5) => {
                        if first_idr_index.is_none() {
                            first_idr_index = Some(index);
                        }
                    }
                    _ => {}
                }
            }

            if first_idr_index.is_some() {
                if !has_sps_before_idr {
                    if let Some(sps) = first_sps {
                        h264_file
                            .write_all(sps)
                            .context("Failed to write SPS to temp H.264 file")?;
                    }
                }

                if !has_pps_before_idr {
                    if let Some(pps) = first_pps {
                        h264_file
                            .write_all(pps)
                            .context("Failed to write PPS to temp H.264 file")?;
                    }
                }
            }

            for packet in &self.video_packets {
                h264_file
                    .write_all(packet.data.as_ref())
                    .context("Failed to write H.264 data to temp file")?;
            }
        }

        // Use FFmpeg to remux H.264 to MP4 (no transcoding).
        // Do not pass rawvideo-specific options (e.g. video_size) to H.264 input.
        let mut command = Command::new(&ffmpeg_cmd);
        command
            .arg("-y")
            .arg("-f")
            .arg("h264")
            .arg("-r")
            .arg(format!("{:.6}", effective_fps))
            .arg("-i")
            .arg(&h264_temp_path)
            // Copy video stream
            .arg("-c:v")
            .arg("copy");

        if self.config.faststart {
            command.arg("-movflags").arg("+faststart");
        }

        let output = command
            .arg(&self.output_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .with_context(|| format!("Failed to run ffmpeg for H.264 remux: {:?}", ffmpeg_cmd))?;

        // Clean up temp file
        let _ = std::fs::remove_file(&h264_temp_path);

        let stderr = String::from_utf8_lossy(&output.stderr);
        if !output.status.success() {
            if self.output_path.exists() {
                let _ = std::fs::remove_file(&self.output_path);
            }
            error!("FFmpeg H.264 remux stderr:\n{}", stderr);
            bail!(
                "ffmpeg failed to remux H.264 to MP4: status {}",
                output.status
            );
        }

        if !stderr.is_empty() {
            debug!("FFmpeg H.264 remux output:\n{}", stderr);
        }

        let metadata = std::fs::metadata(&self.output_path).with_context(|| {
            format!(
                "Missing output MP4 after H.264 remux: {:?}",
                self.output_path
            )
        })?;
        if metadata.len() == 0 {
            bail!(
                "Generated MP4 is empty after H.264 remux: {:?}",
                self.output_path
            );
        }

        info!(
            "H.264 fast mux complete: {:?} ({} frames)",
            self.output_path,
            self.video_packets.len()
        );

        Ok(self.output_path.clone())
    }

    /// Slow path: Transcode MJPEG frames to H.264 and mux to MP4
    #[cfg(feature = "ffmpeg")]
    fn finalize_ffmpeg_mjpeg_transcode(
        &self,
        ffmpeg_cmd: OsString,
        qpc_frequency_f64: f64,
    ) -> Result<PathBuf> {
        let effective_fps = if self.video_packets.len() >= 2 {
            let first_pts = self
                .video_packets
                .first()
                .map(|packet| packet.pts)
                .unwrap_or(0);
            let last_pts = self
                .video_packets
                .last()
                .map(|packet| packet.pts)
                .unwrap_or(first_pts);
            let span_qpc = (last_pts - first_pts).max(1) as f64;
            let span_secs = span_qpc / qpc_frequency_f64;
            if span_secs > 0.0 {
                (self.video_packets.len() as f64 / span_secs).clamp(0.1, self.config.fps.max(1.0))
            } else {
                self.config.fps.max(1.0)
            }
        } else {
            self.config.fps.max(1.0)
        };

        info!(
            "Muxing {} frames with effective input FPS {:.3} (target {:.3}). PPS range: {} - {}",
            self.video_packets.len(),
            effective_fps,
            self.config.fps,
            self.video_packets.first().map(|p| p.pts).unwrap_or(0),
            self.video_packets.last().map(|p| p.pts).unwrap_or(0)
        );

        let mut command = Command::new(&ffmpeg_cmd);
        command
            .arg("-y")
            .arg("-r")
            .arg(format!("{:.6}", effective_fps))
            .arg("-f")
            .arg("mjpeg")
            .arg("-i")
            .arg("pipe:0")
            .arg("-c:v")
            .arg("libx264")
            .arg("-r")
            .arg(format!("{:.6}", effective_fps))
            .arg("-crf")
            .arg("23")
            .arg("-preset")
            .arg("ultrafast")
            .arg("-tune")
            .arg("zerolatency")
            .arg("-pix_fmt")
            .arg("yuv420p");

        if self.config.faststart {
            command.arg("-movflags").arg("+faststart");
        }

        let mut child = command
            .arg(&self.output_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("Failed to launch ffmpeg command: {:?}", ffmpeg_cmd))?;

        let mut written_frames = 0usize;
        {
            let stdin = child
                .stdin
                .as_mut()
                .context("Failed to open ffmpeg stdin")?;

            for packet in &self.video_packets {
                stdin
                    .write_all(packet.data.as_ref())
                    .context("Failed writing MJPEG frame bytes to ffmpeg")?;
                written_frames += 1;
            }
        }

        if written_frames == 0 {
            if self.output_path.exists() {
                let _ = std::fs::remove_file(&self.output_path);
            }
            bail!("No encoded frames available for MP4 generation");
        }

        let output = child
            .wait_with_output()
            .context("Failed waiting for ffmpeg process")?;

        let stderr = String::from_utf8_lossy(&output.stderr);
        if !output.status.success() {
            if self.output_path.exists() {
                let _ = std::fs::remove_file(&self.output_path);
            }
            error!("FFmpeg stderr:\n{}", stderr);
            bail!("ffmpeg failed to generate MP4: status {}", output.status);
        }

        if !stderr.is_empty() {
            debug!("FFmpeg output:\n{}", stderr);
        }

        let metadata = std::fs::metadata(&self.output_path)
            .with_context(|| format!("Missing output MP4 after ffmpeg: {:?}", self.output_path))?;
        if metadata.len() == 0 {
            bail!("Generated MP4 is empty: {:?}", self.output_path);
        }

        info!(
            "FFmpeg MP4 finalized: {:?} ({} frames)",
            self.output_path, written_frames
        );

        Ok(self.output_path.clone())
    }

    #[cfg(feature = "ffmpeg")]
    fn resolve_ffmpeg_command(&self) -> OsString {
        if let Ok(custom) = std::env::var("LITECLIP_FFMPEG_PATH") {
            if !custom.trim().is_empty() {
                return OsString::from(custom);
            }
        }

        if let Ok(cwd) = std::env::current_dir() {
            let candidate = cwd.join("ffmpeg").join("bin").join("ffmpeg.exe");
            if candidate.exists() {
                return candidate.into_os_string();
            }
        }

        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let candidate = exe_dir.join("ffmpeg").join("bin").join("ffmpeg.exe");
                if candidate.exists() {
                    return candidate.into_os_string();
                }
            }
        }

        OsString::from("ffmpeg")
    }

    #[cfg(not(feature = "ffmpeg"))]
    fn create_stub_mp4(&self) -> Result<PathBuf> {
        // Do not create fake/corrupt files when FFmpeg is unavailable.
        // Return a clear actionable error instead.
        if self.output_path.exists() {
            std::fs::remove_file(&self.output_path).with_context(|| {
                format!("Failed to remove stale output file: {:?}", self.output_path)
            })?;
        }

        bail!("Cannot create MP4: FFmpeg feature is disabled. Rebuild with `--features ffmpeg`.")
    }

    /// Get output path
    pub fn output_path(&self) -> &Path {
        &self.output_path
    }
}

/// Calculate start timestamp for clip based on duration
///
/// Returns the QPC timestamp to seek to (nearest keyframe at or before this time).
pub fn calculate_clip_start_pts(newest_pts: i64, duration: std::time::Duration) -> i64 {
    let mut qpc_freq = 10_000_000i64;
    unsafe {
        let _ = windows::Win32::System::Performance::QueryPerformanceFrequency(&mut qpc_freq);
    }
    let duration_qpc = (duration.as_secs_f64() * qpc_freq as f64) as i64;
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
pub fn extract_thumbnail(_packet: &EncodedPacket, output_path: &Path) -> Result<PathBuf> {
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
        use std::env;
        use std::fs;

        let temp_dir = env::temp_dir().join("liteclip_test");
        let result = generate_output_path(&temp_dir);

        assert!(result.is_ok());
        let path = result.unwrap();
        assert!(path.to_string_lossy().ends_with(".mp4"));

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_is_h264_format_4byte_start_code() {
        // 4-byte start code: 0x00 0x00 0x00 0x01 followed by NAL unit
        let h264_data = vec![0x00, 0x00, 0x00, 0x01, 0x67, 0x64, 0x00, 0x1f];
        assert!(is_h264_format(&h264_data));
    }

    #[test]
    fn test_is_h264_format_3byte_start_code() {
        // 3-byte start code: 0x00 0x00 0x01 followed by NAL unit
        let h264_data = vec![0x00, 0x00, 0x01, 0x67, 0x64, 0x00, 0x1f];
        assert!(is_h264_format(&h264_data));
    }

    #[test]
    fn test_is_h264_format_mjpeg() {
        // JPEG starts with 0xFF 0xD8
        let mjpeg_data = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46];
        assert!(!is_h264_format(&mjpeg_data));
    }

    #[test]
    fn test_is_h264_format_too_short() {
        // Too short to be H.264
        let short_data = vec![0x00, 0x00];
        assert!(!is_h264_format(&short_data));
    }
}
