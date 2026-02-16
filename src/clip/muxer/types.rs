//! Auto-generated module
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use crate::encode::{EncodedPacket, StreamType};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
#[cfg(feature = "ffmpeg")]
use std::{
    ffi::OsString,
    io::Write,
    process::{Command, Stdio},
};
use tracing::{debug, error, info, trace, warn};

use super::functions::{
    h264_nal_type, is_h264_format, qpc_delta_to_aligned_pcm_bytes, write_silence_bytes,
    AUDIO_BITRATE, AUDIO_CHANNELS, AUDIO_SAMPLE_RATE,
};
#[cfg(feature = "ffmpeg")]
use crate::buffer::ring::qpc_frequency;

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
        self.audio_packets.sort_by_key(|packet| packet.pts);
        let is_h264 = self
            .video_packets
            .first()
            .map(|p| is_h264_format(&p.data))
            .unwrap_or(false);
        if is_h264 {
            info!("Detected H.264 format - using fast muxing path (no transcoding)");
            return self.finalize_ffmpeg_h264_copy(ffmpeg_cmd, qpc_frequency_f64);
        }
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
        let frame_packets: Vec<&EncodedPacket> = self
            .video_packets
            .iter()
            .filter(|packet| matches!(h264_nal_type(packet.data.as_ref()), Some(1 | 5)))
            .collect();
        let effective_fps = if frame_packets.len() >= 2 {
            let first_pts = frame_packets.first().map(|p| p.pts).unwrap_or(0);
            let last_pts = frame_packets.last().map(|p| p.pts).unwrap_or(first_pts);
            let span_qpc = (last_pts - first_pts).max(1) as f64;
            let span_secs = span_qpc / qpc_frequency_f64;
            if span_secs > 0.0 {
                ((frame_packets.len().saturating_sub(1)) as f64 / span_secs)
                    .clamp(0.1, self.config.fps.max(1.0))
            } else {
                self.config.fps.max(1.0)
            }
        } else {
            self.config.fps.max(1.0)
        };
        info!(
            "Muxing H.264 stream with FPS {:.3} (frame_nals={}, total_nals={})",
            effective_fps,
            frame_packets.len(),
            self.video_packets.len()
        );
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
                        // SPS/PPS must precede first IDR, so we can stop scanning
                        break;
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
        let audio_tracks = self.write_audio_temp_pcm_tracks()?;
        let mut command = Command::new(&ffmpeg_cmd);
        command
            .arg("-y")
            .arg("-f")
            .arg("h264")
            .arg("-r")
            .arg(format!("{:.6}", effective_fps))
            .arg("-i")
            .arg(&h264_temp_path);
        for track in &audio_tracks {
            command
                .arg("-f")
                .arg("s16le")
                .arg("-ar")
                .arg(AUDIO_SAMPLE_RATE.to_string())
                .arg("-ac")
                .arg(AUDIO_CHANNELS.to_string())
                .arg("-i")
                .arg(&track.path);
        }
        command.arg("-map").arg("0:v:0").arg("-c:v").arg("copy");
        Self::append_ffmpeg_audio_track_mapping_args(&mut command, &audio_tracks);
        if self.config.faststart {
            command.arg("-movflags").arg("+faststart");
        }
        let output = command
            .arg(&self.output_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .with_context(|| format!("Failed to run ffmpeg for H.264 remux: {:?}", ffmpeg_cmd))?;
        let _ = std::fs::remove_file(&h264_temp_path);
        for track in &audio_tracks {
            let _ = std::fs::remove_file(&track.path);
        }
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
    #[cfg(feature = "ffmpeg")]
    fn write_audio_temp_pcm_tracks(&self) -> Result<Vec<AudioTrackInput>> {
        if self.audio_packets.is_empty() {
            if self.config.expect_audio {
                warn!(
                    "No captured audio packets found for this clip; generating silent fallback track"
                );
                if let Some(path) = self.write_silent_audio_temp_pcm("system")? {
                    return Ok(vec![AudioTrackInput {
                        path,
                        title: "system",
                    }]);
                }
            }
            return Ok(Vec::new());
        }
        let mut system_packets: Vec<&EncodedPacket> = Vec::new();
        let mut mic_packets: Vec<&EncodedPacket> = Vec::new();
        for packet in &self.audio_packets {
            match packet.stream {
                StreamType::SystemAudio => system_packets.push(packet),
                StreamType::Microphone => mic_packets.push(packet),
                StreamType::Video => {}
            }
        }
        let video_pts_range = (
            self.video_packets.first().map(|p| p.pts).unwrap_or(0),
            self.video_packets.last().map(|p| p.pts).unwrap_or(0),
        );
        info!(
            "Audio packet breakdown: {} system, {} mic (total audio={}). Video PTS range: {}..{}",
            system_packets.len(),
            mic_packets.len(),
            self.audio_packets.len(),
            video_pts_range.0,
            video_pts_range.1,
        );
        if !system_packets.is_empty() {
            let sys_first = system_packets.first().map(|p| p.pts).unwrap_or(0);
            let sys_last = system_packets.last().map(|p| p.pts).unwrap_or(0);
            info!("System audio PTS range: {}..{}", sys_first, sys_last);
        }
        if !mic_packets.is_empty() {
            let mic_first = mic_packets.first().map(|p| p.pts).unwrap_or(0);
            let mic_last = mic_packets.last().map(|p| p.pts).unwrap_or(0);
            info!("Microphone audio PTS range: {}..{}", mic_first, mic_last);
        } else if self.config.expect_audio && !system_packets.is_empty() {
            warn!(
                "Microphone capture was enabled but no mic packets found in clip \
                 (system audio has {} packets). Mic capture may have failed silently \
                 or mic packets were evicted from the buffer.",
                system_packets.len()
            );
        }
        let mut tracks = Vec::new();
        if let Some(path) = self.write_stream_audio_temp_pcm(&system_packets, "system")? {
            tracks.push(AudioTrackInput {
                path,
                title: "system",
            });
        }
        if let Some(path) = self.write_stream_audio_temp_pcm(&mic_packets, "mic")? {
            tracks.push(AudioTrackInput {
                path,
                title: "microphone",
            });
        }
        if tracks.is_empty() && self.config.expect_audio {
            warn!(
                "Audio packets were present but unusable after alignment; generating silent fallback track"
            );
            if let Some(path) = self.write_silent_audio_temp_pcm("system")? {
                tracks.push(AudioTrackInput {
                    path,
                    title: "system",
                });
            }
        }
        Ok(tracks)
    }
    #[cfg(feature = "ffmpeg")]
    fn write_stream_audio_temp_pcm(
        &self,
        packets: &[&EncodedPacket],
        stream_suffix: &str,
    ) -> Result<Option<PathBuf>> {
        if packets.is_empty() {
            return Ok(None);
        }
        let audio_temp_path = self
            .output_path
            .with_extension(format!("{stream_suffix}.pcm"));
        let mut audio_file = std::fs::File::create(&audio_temp_path)
            .with_context(|| format!("Failed to create temp PCM file: {:?}", audio_temp_path))?;
        let bytes_per_frame = AUDIO_CHANNELS as usize * 2;
        let bytes_per_second = AUDIO_SAMPLE_RATE as f64 * bytes_per_frame as f64;
        let qpc_freq = qpc_frequency() as f64;
        let video_start_pts = self.video_packets.first().map(|p| p.pts).unwrap_or(0);
        let mut bytes_written = 0usize;
        let mut payload_bytes_written = 0usize;
        let mut silence_padding_bytes = 0usize;
        let mut trimmed_overlap_bytes = 0usize;
        for packet in packets {
            let data = packet.data.as_ref();
            let aligned_len = data.len().saturating_sub(data.len() % bytes_per_frame);
            if aligned_len == 0 {
                continue;
            }
            let mut packet_start_bytes = qpc_delta_to_aligned_pcm_bytes(
                packet.pts.saturating_sub(video_start_pts),
                qpc_freq,
                bytes_per_second,
                bytes_per_frame,
            );
            let mut skip_bytes = 0usize;
            if packet_start_bytes < 0 {
                let trim = ((-packet_start_bytes) as usize).min(aligned_len);
                let trim_aligned = trim.saturating_sub(trim % bytes_per_frame);
                skip_bytes = trim_aligned;
                packet_start_bytes = 0;
            }
            let current_timeline_bytes = bytes_written as i64;
            if packet_start_bytes > current_timeline_bytes {
                let gap_bytes = (packet_start_bytes - current_timeline_bytes) as usize;
                write_silence_bytes(&mut audio_file, gap_bytes)?;
                bytes_written += gap_bytes;
                silence_padding_bytes += gap_bytes;
            } else if packet_start_bytes < current_timeline_bytes {
                let overlap = (current_timeline_bytes - packet_start_bytes) as usize;
                let overlap_aligned = overlap.saturating_sub(overlap % bytes_per_frame);
                skip_bytes = skip_bytes.saturating_add(overlap_aligned).min(aligned_len);
            }
            if skip_bytes >= aligned_len {
                trimmed_overlap_bytes = trimmed_overlap_bytes.saturating_add(aligned_len);
                continue;
            }
            audio_file
                .write_all(&data[skip_bytes..aligned_len])
                .context("Failed writing PCM audio data to temp file")?;
            let written = aligned_len - skip_bytes;
            bytes_written += written;
            payload_bytes_written += written;
            trimmed_overlap_bytes += skip_bytes;
        }
        if bytes_written == 0 {
            let _ = std::fs::remove_file(&audio_temp_path);
            return Ok(None);
        }
        info!(
            "Prepared PCM audio input for muxing ({}): {} packets, {} bytes (payload={}, silence_padding={}, trimmed_overlap={})",
            stream_suffix, packets.len(), bytes_written, payload_bytes_written,
            silence_padding_bytes, trimmed_overlap_bytes
        );
        Ok(Some(audio_temp_path))
    }
    #[cfg(feature = "ffmpeg")]
    fn write_silent_audio_temp_pcm(&self, stream_suffix: &str) -> Result<Option<PathBuf>> {
        let audio_temp_path = self
            .output_path
            .with_extension(format!("{stream_suffix}.pcm"));
        let mut audio_file = std::fs::File::create(&audio_temp_path)
            .with_context(|| format!("Failed to create temp PCM file: {:?}", audio_temp_path))?;
        let duration_secs = 0.2f64;
        let total_frames = (duration_secs * AUDIO_SAMPLE_RATE as f64).round() as usize;
        let bytes_per_frame = AUDIO_CHANNELS as usize * 2;
        let zero_chunk_frames = 2048usize;
        let zero_chunk = vec![0u8; zero_chunk_frames * bytes_per_frame];
        let mut remaining_frames = total_frames;
        let mut bytes_written = 0usize;
        while remaining_frames > 0 {
            let chunk_frames = remaining_frames.min(zero_chunk_frames);
            let chunk_bytes = chunk_frames * bytes_per_frame;
            audio_file
                .write_all(&zero_chunk[..chunk_bytes])
                .context("Failed writing silent fallback PCM data")?;
            bytes_written += chunk_bytes;
            remaining_frames -= chunk_frames;
        }
        info!(
            "Prepared silent PCM fallback for muxing ({}): {:.3}s ({} bytes)",
            stream_suffix, duration_secs, bytes_written
        );
        Ok(Some(audio_temp_path))
    }
    #[cfg(feature = "ffmpeg")]
    fn append_ffmpeg_audio_track_mapping_args(
        command: &mut Command,
        audio_tracks: &[AudioTrackInput],
    ) {
        if audio_tracks.is_empty() {
            command.arg("-an");
            return;
        }
        command
            .arg("-c:a")
            .arg("aac")
            .arg("-b:a")
            .arg(AUDIO_BITRATE);
        if audio_tracks.len() == 1 {
            command.arg("-map").arg("1:a:0");
            return;
        }
        let mut input_labels = String::new();
        for index in 0..audio_tracks.len() {
            let input_index = index + 1;
            input_labels.push_str(&format!("[{input_index}:a:0]"));
        }
        let filter = format!(
            "{input_labels}amix=inputs={}:normalize=1[aout]",
            audio_tracks.len()
        );
        info!(
            "Mixing {} audio tracks into one output stream: {}",
            audio_tracks.len(),
            audio_tracks
                .iter()
                .map(|track| track.title)
                .collect::<Vec<_>>()
                .join(", ")
        );
        command
            .arg("-filter_complex")
            .arg(filter)
            .arg("-map")
            .arg("[aout]");
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
                ((self.video_packets.len().saturating_sub(1)) as f64 / span_secs)
                    .clamp(0.1, self.config.fps.max(1.0))
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
        let audio_tracks = self.write_audio_temp_pcm_tracks()?;
        let mut command = Command::new(&ffmpeg_cmd);
        command
            .arg("-y")
            .arg("-r")
            .arg(format!("{:.6}", effective_fps))
            .arg("-f")
            .arg("mjpeg")
            .arg("-i")
            .arg("pipe:0");
        for track in &audio_tracks {
            command
                .arg("-f")
                .arg("s16le")
                .arg("-ar")
                .arg(AUDIO_SAMPLE_RATE.to_string())
                .arg("-ac")
                .arg(AUDIO_CHANNELS.to_string())
                .arg("-i")
                .arg(&track.path);
        }
        command
            .arg("-map")
            .arg("0:v:0")
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
        Self::append_ffmpeg_audio_track_mapping_args(&mut command, &audio_tracks);
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
        drop(child.stdin.take());
        if written_frames == 0 {
            if self.output_path.exists() {
                let _ = std::fs::remove_file(&self.output_path);
            }
            bail!("No encoded frames available for MP4 generation");
        }
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let stdout_thread = std::thread::spawn(move || -> Vec<u8> {
            let mut buf = Vec::new();
            if let Some(mut out) = stdout {
                use std::io::Read;
                let _ = out.read_to_end(&mut buf);
            }
            buf
        });
        let stderr_thread = std::thread::spawn(move || -> Vec<u8> {
            let mut buf = Vec::new();
            if let Some(mut err) = stderr {
                use std::io::Read;
                let _ = err.read_to_end(&mut buf);
            }
            buf
        });
        let status = child.wait().context("Failed waiting for ffmpeg process")?;
        for track in &audio_tracks {
            let _ = std::fs::remove_file(&track.path);
        }
        let _stdout_data = stdout_thread.join().unwrap_or_default();
        let stderr_data = stderr_thread.join().unwrap_or_default();
        let stderr_str = String::from_utf8_lossy(&stderr_data);
        if !status.success() {
            if self.output_path.exists() {
                let _ = std::fs::remove_file(&self.output_path);
            }
            error!("FFmpeg stderr:\n{}", stderr_str);
            bail!("ffmpeg failed to generate MP4: status {}", status);
        }
        if !stderr_str.is_empty() {
            debug!("FFmpeg output:\n{}", stderr_str);
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
#[cfg(feature = "ffmpeg")]
struct AudioTrackInput {
    path: PathBuf,
    title: &'static str,
}
