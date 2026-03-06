//! Auto-generated module
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use super::functions::{
    h264_nal_type, hevc_nal_type, qpc_delta_to_aligned_pcm_bytes,
    write_silence_bytes, AUDIO_BITRATE, AUDIO_CHANNELS, AUDIO_SAMPLE_RATE,
};
use super::ffmpeg_muxer::FfmpegMuxer;
use crate::buffer::ring::qpc_frequency;
#[cfg(feature = "ffmpeg")]
use crate::encode::hw_encoder::functions::PROCESS_CREATION_FLAGS;
use crate::encode::{EncodedPacket, StreamType};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
#[cfg(feature = "ffmpeg")]
use std::{
    ffi::OsString,
    io::Write,
    os::windows::process::CommandExt,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use tracing::{debug, error, info, trace, warn};

#[cfg(feature = "ffmpeg")]
struct AudioTrackInput {
    path: PathBuf,
    title: &'static str,
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
    /// Buffered audio packets (PCM S16LE) used for MP4 generation at finalize()
    #[cfg(feature = "ffmpeg")]
    audio_packets: Vec<EncodedPacket>,
}
impl Muxer {
    /// Create new muxer for output path
    pub fn new(output_path: &Path, config: &MuxerConfig) -> Result<Self> {
        let path = output_path.to_path_buf();
        info!("Creating MP4 muxer for: {:?}", path);
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
                audio_packets: Vec::new(),
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
    pub fn write_audio_packet(&mut self, packet: &EncodedPacket) -> Result<()> {
        #[cfg(feature = "ffmpeg")]
        {
            self.audio_packets.push(packet.clone());
            Ok(())
        }
        #[cfg(not(feature = "ffmpeg"))]
        {
            trace!(
                "Stub: Received audio packet (size={}, pts={}, stream={:?})",
                packet.data.len(),
                packet.pts,
                packet.stream
            );
            Ok(())
        }
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
        if self.video_packets.is_empty() {
            bail!("No video packets available for MP4 generation");
        }
        
        self.video_packets.sort_by_key(|packet| packet.pts);
        self.audio_packets.sort_by_key(|packet| packet.pts);

        let mut muxer = FfmpegMuxer::new(
            &self.output_path,
            &self.config.video_codec,
            self.config.width,
            self.config.height,
            self.config.fps
        )?;

        muxer.write_packets(&self.video_packets, &self.audio_packets)?;

        info!("MP4 finalized natively: {:?}", self.output_path);
        Ok(self.output_path)
    }
    /// Get output path
    pub fn output_path(&self) -> &Path {
        &self.output_path
    }
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
    /// If true, ensure the output contains an audio track even when no captured audio packets exist.
    pub expect_audio: bool,
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
            expect_audio: false,
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
    /// Mark whether the recorder expected audio input for this clip.
    pub fn with_expect_audio(mut self, expect_audio: bool) -> Self {
        self.expect_audio = expect_audio;
        self
    }
}
#[cfg(not(feature = "ffmpeg"))]
impl Muxer {
    fn create_stub_mp4(&self) -> Result<PathBuf> {
        if self.output_path.exists() {
            std::fs::remove_file(&self.output_path).with_context(|| {
                format!("Failed to remove stale output file: {:?}", self.output_path)
            })?;
        }
        bail!("Cannot create MP4: FFmpeg feature is disabled. Rebuild with `--features ffmpeg`.")
    }
}
