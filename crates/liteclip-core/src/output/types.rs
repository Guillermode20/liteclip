#[cfg(any(feature = "ffmpeg", feature = "ffmpeg-cli"))]
use super::functions::{h264_nal_type, hevc_nal_type};
#[cfg(feature = "ffmpeg")]
use super::mp4::FfmpegMuxer;
use crate::encode::{EncodedPacket, StreamType};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use tracing::{info, trace, warn};

/// MP4 muxer for combining encoded packets into a video file.
///
/// Handles muxing of video and audio streams into an MP4 container
/// using FFmpeg.
pub struct Muxer {
    /// Path to the output MP4 file.
    output_path: PathBuf,
    /// Muxer configuration.
    #[allow(dead_code)]
    config: MuxerConfig,
    /// Stub mode when no FFmpeg backend is enabled.
    #[cfg(not(any(feature = "ffmpeg", feature = "ffmpeg-cli")))]
    #[allow(dead_code)]
    stub_mode: bool,
}

impl Muxer {
    #[cfg(any(feature = "ffmpeg", feature = "ffmpeg-cli"))]
    fn detect_video_codec(video_packets: &[&EncodedPacket], fallback: &str) -> String {
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

            if matches!(hevc_nal_type(data), Some(32..=34)) {
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
        #[cfg(any(feature = "ffmpeg", feature = "ffmpeg-cli"))]
        {
            Ok(Self {
                output_path: path,
                config: config.clone(),
            })
        }
        #[cfg(not(any(feature = "ffmpeg", feature = "ffmpeg-cli")))]
        {
            tracing::warn!("No FFmpeg backend enabled - muxer running in stub mode");
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
        #[cfg(any(feature = "ffmpeg", feature = "ffmpeg-cli"))]
        {
            let _ = packet;
            bail!("Incremental packet buffering is no longer supported; use Muxer::mux_clip")
        }
        #[cfg(not(any(feature = "ffmpeg", feature = "ffmpeg-cli")))]
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
        #[cfg(any(feature = "ffmpeg", feature = "ffmpeg-cli"))]
        {
            let _ = packet;
            bail!("Incremental packet buffering is no longer supported; use Muxer::mux_clip")
        }
        #[cfg(not(any(feature = "ffmpeg", feature = "ffmpeg-cli")))]
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
        #[cfg(any(feature = "ffmpeg", feature = "ffmpeg-cli"))]
        {
            bail!("No packet set provided for MP4 generation")
        }
        #[cfg(not(any(feature = "ffmpeg", feature = "ffmpeg-cli")))]
        {
            tracing::warn!("FFmpeg backend disabled - cannot produce MP4");
            self.create_stub_mp4()
        }
    }

    #[cfg(feature = "ffmpeg")]
    pub fn mux_clip(
        output_path: &Path,
        config: &MuxerConfig,
        packets: &[EncodedPacket],
    ) -> Result<PathBuf> {
        let mut raw_video_packets: Vec<&EncodedPacket> = packets
            .iter()
            .filter(|packet| matches!(packet.stream, StreamType::Video))
            .collect();
        if raw_video_packets.is_empty() {
            bail!("No video packets available for MP4 generation");
        }

        let mut audio_packets: Vec<&EncodedPacket> = packets
            .iter()
            .filter(|packet| {
                matches!(
                    packet.stream,
                    StreamType::SystemAudio | StreamType::Microphone
                )
            })
            .collect();

        raw_video_packets.sort_by_key(|packet| packet.pts);
        audio_packets.sort_by_key(|packet| packet.pts);

        let normalized_video_storage = normalize_video_packets_for_mp4(&raw_video_packets);
        let video_packets: Vec<&EncodedPacket> = normalized_video_storage.iter().collect();
        if video_packets.is_empty() {
            bail!("No muxable video packets available for MP4 generation");
        }

        let detected_video_codec = Self::detect_video_codec(&video_packets, &config.video_codec);
        if detected_video_codec != config.video_codec {
            warn!(
                "Muxer video codec override: configured={}, detected={} from buffered packets",
                config.video_codec, detected_video_codec
            );
        }

        info!("Writing MP4 to {:?}", output_path);

        let mut muxer = FfmpegMuxer::new(
            output_path,
            &detected_video_codec,
            config.width,
            config.height,
            config.fps,
            config,
        )?;

        let (video_count, audio_count) = muxer.write_packets(&video_packets, &audio_packets)?;
        drop(muxer);

        info!(
            "MP4 finalized natively: {:?} ({} video packets, {} audio packets)",
            output_path, video_count, audio_count
        );
        Ok(output_path.to_path_buf())
    }

    #[cfg(all(feature = "ffmpeg-cli", not(feature = "ffmpeg")))]
    pub fn mux_clip(
        output_path: &Path,
        config: &MuxerConfig,
        packets: &[EncodedPacket],
    ) -> Result<PathBuf> {
        mux_clip_via_ffmpeg_cli(output_path, config, packets)
    }

    #[cfg(not(any(feature = "ffmpeg", feature = "ffmpeg-cli")))]
    pub fn mux_clip(
        _output_path: &Path,
        _config: &MuxerConfig,
        _packets: &[EncodedPacket],
    ) -> Result<PathBuf> {
        bail!("FFmpeg backend disabled; use `--features ffmpeg` or `--features ffmpeg-cli`")
    }
}

#[cfg(any(feature = "ffmpeg", feature = "ffmpeg-cli"))]
fn normalize_video_packets_for_mp4(video_packets: &[&EncodedPacket]) -> Vec<EncodedPacket> {
    let mut normalized = Vec::with_capacity(video_packets.len());
    let mut pending_param_sets: Vec<&EncodedPacket> = Vec::new();
    let mut merged_prefix_groups = 0usize;
    let mut merged_param_set_packets = 0usize;
    let mut dropped_param_set_packets = 0usize;

    for packet in video_packets {
        if is_parameter_set_packet(packet) {
            pending_param_sets.push(*packet);
            continue;
        }

        if pending_param_sets.is_empty() {
            normalized.push((*packet).clone());
            continue;
        }

        let same_timestamp = pending_param_sets
            .iter()
            .all(|param| param.pts == packet.pts && param.dts == packet.dts);

        if same_timestamp {
            let merged_len = pending_param_sets
                .iter()
                .map(|param| param.data.len())
                .sum::<usize>()
                .saturating_add(packet.data.len());
            let mut merged_data = Vec::with_capacity(merged_len);
            for param in &pending_param_sets {
                merged_data.extend_from_slice(param.data.as_ref());
            }
            merged_data.extend_from_slice(packet.data.as_ref());

            let mut merged_packet = (*packet).clone();
            merged_packet.data = merged_data.into();
            normalized.push(merged_packet);

            merged_prefix_groups += 1;
            merged_param_set_packets += pending_param_sets.len();
            pending_param_sets.clear();
        } else {
            dropped_param_set_packets += pending_param_sets.len();
            warn!(
                "Dropping {} standalone parameter-set packets before video packet at pts={} because timestamps differ",
                pending_param_sets.len(),
                packet.pts
            );
            pending_param_sets.clear();
            normalized.push((*packet).clone());
        }
    }

    if !pending_param_sets.is_empty() {
        dropped_param_set_packets += pending_param_sets.len();
        warn!(
            "Dropping {} trailing standalone parameter-set packets with no following video frame",
            pending_param_sets.len()
        );
    }

    if merged_param_set_packets > 0 {
        info!(
            "Merged {} standalone parameter-set packets into {} MP4 video samples",
            merged_param_set_packets, merged_prefix_groups
        );
    }

    if dropped_param_set_packets > 0 {
        warn!(
            "Dropped {} standalone parameter-set packets from MP4 timed samples",
            dropped_param_set_packets
        );
    }

    normalized
}

#[cfg(any(feature = "ffmpeg", feature = "ffmpeg-cli"))]
fn is_parameter_set_packet(packet: &EncodedPacket) -> bool {
    if !matches!(packet.stream, StreamType::Video) {
        return false;
    }

    let data = packet.data.as_ref();
    matches!(h264_nal_type(data), Some(7 | 8)) || matches!(hevc_nal_type(data), Some(32..=34))
}

/// Configuration for the MP4 muxer.
///
/// Controls output file properties including resolution, codec, and audio settings.
#[derive(Debug, Clone)]
pub struct MuxerConfig {
    /// Video width in pixels.
    pub width: u32,
    /// Video height in pixels.
    pub height: u32,
    /// Video codec (e.g., "h264", "hevc").
    pub video_codec: String,
    /// Frames per second.
    pub fps: f64,
    /// Output file path.
    pub output_path: PathBuf,
    /// Whether to enable faststart for web streaming.
    pub faststart: bool,
    /// Whether to expect audio streams.
    pub expect_audio: bool,
}

impl MuxerConfig {
    /// Creates a new muxer configuration.
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

    /// Sets the video codec.
    pub fn with_video_codec(mut self, codec: impl Into<String>) -> Self {
        self.video_codec = codec.into();
        self
    }

    /// Sets the faststart option.
    pub fn with_faststart(mut self, faststart: bool) -> Self {
        self.faststart = faststart;
        self
    }

    /// Sets whether audio is expected.
    pub fn with_expect_audio(mut self, expect_audio: bool) -> Self {
        self.expect_audio = expect_audio;
        self
    }
}

#[cfg(not(any(feature = "ffmpeg", feature = "ffmpeg-cli")))]
impl Muxer {
    fn create_stub_mp4(&self) -> Result<PathBuf> {
        if self.output_path.exists() {
            std::fs::remove_file(&self.output_path).with_context(|| {
                format!("Failed to remove stale output file: {:?}", self.output_path)
            })?;
        }
        bail!("Cannot create MP4: no FFmpeg backend. Rebuild with `--features ffmpeg` or `--features ffmpeg-cli`.")
    }
}

#[cfg(all(feature = "ffmpeg-cli", not(feature = "ffmpeg")))]
fn mux_clip_via_ffmpeg_cli(
    output_path: &Path,
    config: &MuxerConfig,
    packets: &[EncodedPacket],
) -> Result<PathBuf> {
    use std::fs::File;
    use std::io::Write;
    use std::process::Command;

    use super::functions::ffmpeg_executable_path;

    #[cfg(target_os = "windows")]
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let mut raw_video_packets: Vec<&EncodedPacket> = packets
        .iter()
        .filter(|packet| matches!(packet.stream, StreamType::Video))
        .collect();
    if raw_video_packets.is_empty() {
        bail!("No video packets available for MP4 generation");
    }

    let audio_packets: Vec<&EncodedPacket> = packets
        .iter()
        .filter(|packet| {
            matches!(
                packet.stream,
                StreamType::SystemAudio | StreamType::Microphone
            )
        })
        .collect();

    if config.expect_audio && !audio_packets.is_empty() {
        bail!(
            "ffmpeg-cli backend: muxing audio into MP4 is not implemented; use the SDK backend (`--features ffmpeg`) or disable audio"
        );
    }

    raw_video_packets.sort_by_key(|packet| packet.pts);

    let normalized_video_storage = normalize_video_packets_for_mp4(&raw_video_packets);
    let video_packets: Vec<&EncodedPacket> = normalized_video_storage.iter().collect();
    if video_packets.is_empty() {
        bail!("No muxable video packets available for MP4 generation");
    }

    let detected_video_codec = Muxer::detect_video_codec(&video_packets, &config.video_codec);
    if detected_video_codec != config.video_codec {
        warn!(
            "Muxer video codec override: configured={}, detected={} from buffered packets",
            config.video_codec, detected_video_codec
        );
    }

    let tmp = std::env::temp_dir().join(format!(
        "liteclip-mux-{}-{}.bin",
        std::process::id(),
        chrono::Utc::now().timestamp_millis()
    ));
    {
        let mut f = File::create(&tmp).with_context(|| format!("Failed to create {:?}", tmp))?;
        for p in &video_packets {
            f.write_all(p.data.as_ref())?;
        }
    }

    let fmt = if detected_video_codec == "h264" {
        "h264"
    } else {
        "hevc"
    };
    let ffmpeg = ffmpeg_executable_path();
    let mut cmd = Command::new(&ffmpeg);
    cmd.arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-y")
        .arg("-f")
        .arg(fmt)
        .arg("-i")
        .arg(&tmp)
        .arg("-c:v")
        .arg("copy");
    if config.faststart {
        cmd.arg("-movflags").arg("+faststart");
    }
    cmd.arg(output_path);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let status = cmd
        .status()
        .with_context(|| format!("Failed to spawn ffmpeg mux via {:?}", ffmpeg))?;
    let _ = std::fs::remove_file(&tmp);
    if !status.success() {
        bail!("ffmpeg mux to {:?} failed with status {:?}", output_path, status);
    }
    info!(
        "MP4 finalized via ffmpeg CLI: {:?} ({} video packets)",
        output_path,
        video_packets.len()
    );
    Ok(output_path.to_path_buf())
}
