use super::{functions::{h264_nal_type, hevc_nal_type}, mp4::FfmpegMuxer};
use crate::encode::{EncodedPacket, StreamType};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use tracing::{info, trace, warn};

pub struct Muxer {
    output_path: PathBuf,
    #[allow(dead_code)]
    config: MuxerConfig,
    #[cfg(not(feature = "ffmpeg"))]
    #[allow(dead_code)]
    stub_mode: bool,
    #[cfg(feature = "ffmpeg")]
    video_packets: Vec<EncodedPacket>,
    #[cfg(feature = "ffmpeg")]
    audio_packets: Vec<EncodedPacket>,
}

impl Muxer {
    fn detect_video_codec(video_packets: &[EncodedPacket], fallback: &str) -> String {
        let mut saw_h264_parameter_sets = false;
        let mut saw_hevc_parameter_sets = false;

        for packet in video_packets {
            let data = packet.data.as_ref();

            match h264_nal_type(data) {
                Some(7 | 8) => saw_h264_parameter_sets = true,
                Some(1 | 5) if saw_hevc_parameter_sets => {}
                Some(1 | 5) => return "h264".to_string(),
                _ => {}
            }

            if matches!(hevc_nal_type(data), Some(32 | 33 | 34)) {
                saw_hevc_parameter_sets = true;
            }

            if saw_h264_parameter_sets {
                return "h264".to_string();
            }

            if saw_hevc_parameter_sets {
                return "hevc".to_string();
            }
        }

        fallback.to_string()
    }

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

        let detected_video_codec =
            Self::detect_video_codec(&self.video_packets, &self.config.video_codec);
        if detected_video_codec != self.config.video_codec {
            warn!(
                "Muxer video codec override: configured={}, detected={} from buffered packets",
                self.config.video_codec, detected_video_codec
            );
        }

        let mut muxer = FfmpegMuxer::new(
            &self.output_path,
            &detected_video_codec,
            self.config.width,
            self.config.height,
            self.config.fps,
            &self.config,
        )?;

        muxer.write_packets(&self.video_packets, &self.audio_packets)?;

        info!("MP4 finalized natively: {:?}", self.output_path);
        Ok(self.output_path)
    }

    pub fn output_path(&self) -> &Path {
        &self.output_path
    }
}

#[derive(Debug, Clone)]
pub struct MuxerConfig {
    pub width: u32,
    pub height: u32,
    pub video_codec: String,
    pub fps: f64,
    pub output_path: PathBuf,
    pub faststart: bool,
    pub expect_audio: bool,
}

impl MuxerConfig {
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

    pub fn with_video_codec(mut self, codec: impl Into<String>) -> Self {
        self.video_codec = codec.into();
        self
    }

    pub fn with_faststart(mut self, faststart: bool) -> Self {
        self.faststart = faststart;
        self
    }

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
