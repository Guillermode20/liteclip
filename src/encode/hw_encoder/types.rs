//! Auto-generated module
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use super::functions::{
    find_annexb_start_code, h264_nal_type, h264_nonidr_is_intra_slice, hevc_nal_type, query_qpc,
    resolve_ffmpeg_command, PROCESS_CREATION_FLAGS,
};
use crate::buffer::ring::qpc_frequency;
use crate::config::{Codec, QualityPreset, RateControl};
use crate::encode::frame_writer::AsyncFrameWriter;
use crate::encode::{EncodedPacket, EncoderConfig, StreamType};
use anyhow::{Context, Result};
use bytes::{Buf, BytesMut};
use crossbeam::channel::{bounded, Receiver, Sender};
use std::io::{BufRead, BufReader};
use std::os::windows::process::CommandExt;
use std::process::{ChildStdin, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tracing::{debug, enabled, error, info, warn, Level};

/// QSV encoder wrapper (Intel)
pub struct QsvEncoder {
    pub(super) base: HardwareEncoderBase,
}
impl QsvEncoder {
    pub fn new(config: &EncoderConfig) -> Result<Self> {
        debug!("Creating QSV encoder");
        let encoder_name = match config.codec {
            Codec::H264 => "h264_qsv",
            Codec::H265 => "hevc_qsv",
            Codec::Av1 => anyhow::bail!("AV1 is not supported by QSV path in this build"),
        };
        let base = HardwareEncoderBase::new(config, encoder_name)?;
        Ok(Self { base })
    }
}
/// Managed FFmpeg process handle that ensures proper cleanup on drop.
/// Encapsulates the child process, stdin, and reader threads.
pub struct ManagedFfmpegProcess {
    /// The FFmpeg child process
    pub(super) child: std::process::Child,
    /// Stdin handle (only present in CPU readback mode)
    pub(super) stdin: Option<ChildStdin>,
    /// Handle for the stdout reader thread (Option allows take() in Drop)
    pub(super) stdout_reader: Option<thread::JoinHandle<()>>,
    /// Handle for the stderr reader thread (Option allows take() in Drop)
    pub(super) stderr_reader: Option<thread::JoinHandle<()>>,
}
impl ManagedFfmpegProcess {
    /// Create a new managed FFmpeg process from the given child process and thread handles.
    pub fn new(
        child: std::process::Child,
        stdin: Option<ChildStdin>,
        stdout_reader: thread::JoinHandle<()>,
        stderr_reader: thread::JoinHandle<()>,
    ) -> Self {
        Self {
            child,
            stdin,
            stdout_reader: Some(stdout_reader),
            stderr_reader: Some(stderr_reader),
        }
    }
    /// Get a mutable reference to stdin (if available)
    pub fn stdin_mut(&mut self) -> Option<&mut ChildStdin> {
        self.stdin.as_mut()
    }
    /// Check if the process is still running
    pub fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.child.try_wait()
    }
}
/// AMF encoder wrapper (AMD)
pub struct AmfEncoder {
    pub(super) base: HardwareEncoderBase,
}
impl AmfEncoder {
    pub fn new(config: &EncoderConfig) -> Result<Self> {
        debug!("Creating AMF encoder");
        let encoder_name = match config.codec {
            Codec::H264 => "h264_amf",
            Codec::H265 => "hevc_amf",
            Codec::Av1 => "av1_amf",
        };
        let base = HardwareEncoderBase::new(config, encoder_name)?;
        Ok(Self { base })
    }
}
/// Base hardware encoder using FFmpeg CLI
pub struct HardwareEncoderBase {
    pub(crate) config: EncoderConfig,
    encoder_name: String,
    pub(crate) packet_rx: Receiver<EncodedPacket>,
    pub(crate) packet_tx: Sender<EncodedPacket>,
    frame_count: u64,
    pub(crate) running: bool,
    /// Managed FFmpeg process handle (replaces individual Option fields)
    pub(super) ffmpeg: Option<ManagedFfmpegProcess>,
    width: u32,
    height: u32,
    /// Channel sender for frame metadata (timestamps) to output reader
    frame_meta_tx: Option<Sender<FrameMetadata>>,
    /// Async writer for non-blocking stdin writes (CPU readback mode)
    pub(crate) async_writer: Option<AsyncFrameWriter>,
    /// Dropped frame counter for async writer
    dropped_frames: u64,
}
impl HardwareEncoderBase {
    /// Create new hardware encoder with FFmpeg CLI
    pub fn new(config: &EncoderConfig, encoder_name: &str) -> Result<Self> {
        let ffmpeg_cmd = resolve_ffmpeg_command();
        info!(
            "Creating {} encoder with FFmpeg: {}",
            encoder_name, ffmpeg_cmd
        );
        let (width, height) = if config.use_native_resolution {
            (0, 0)
        } else {
            config.resolution
        };
        let (packet_tx, packet_rx) = bounded(64);
        Ok(Self {
            config: config.clone(),
            encoder_name: encoder_name.to_string(),
            packet_rx,
            packet_tx,
            frame_count: 0,
            running: false,
            ffmpeg: None,
            width,
            height,
            frame_meta_tx: None,
            async_writer: None,
            dropped_frames: 0,
        })
    }
    /// Initialize the FFmpeg process with hardware encoder settings
    pub(crate) fn init_ffmpeg(&mut self, width: u32, height: u32) -> Result<()> {
        let ffmpeg_cmd = resolve_ffmpeg_command();
        let encoder_name = self.encoder_name.as_str();
        let (out_w, out_h) = if !self.config.use_native_resolution
            && self.config.resolution.0 > 0
            && self.config.resolution.1 > 0
        {
            self.config.resolution
        } else {
            (width, height)
        };
        let mut cmd = Command::new(&ffmpeg_cmd);
        cmd.arg("-y");
        // Discard corrupted frames (gray frames at startup from ddagrab initialization)
        cmd.arg("-fflags").arg("+discardcorrupt");
        // Setup video filters (scaling and format conversion if needed)
        // Note: For D3D11 zero-copy hardware encoders (like AMF over ddagrab), we must AV_HWFRAME_RESTRICT
        // filters like `scale` without first applying `hwdownload` or converting back, else they will error with
        // "Error reinitializing filters! Function not implemented".
        // Therefore, if resolution reduction is needed on a zero-copy hw-encoder, we must fallback to CPU or use a DXGI shader in the future.
        let mut video_filters: Vec<String> = Vec::new();
        if self.config.use_cpu_readback {
            cmd.arg("-f")
                .arg("rawvideo")
                .arg("-pix_fmt")
                .arg("bgra")
                .arg("-s")
                .arg(format!("{}x{}", width, height))
                .arg("-r")
                .arg(self.config.framerate.to_string())
                .arg("-i")
                .arg("pipe:0");

            if out_w != width || out_h != height {
                video_filters.push(format!("scale={}:{}", out_w, out_h));
            }
        } else {
            cmd.arg("-f")
                .arg("lavfi")
                .arg("-probesize")
                .arg("5M")
                .arg("-analyzeduration")
                .arg("500000")
                .arg("-i")
                .arg(format!(
                    "ddagrab=output_idx={}:framerate={}",
                    self.config.output_index, self.config.framerate
                ));

            // For Desktop Grab + D3D11 zero copy, scaling is not supported natively by standard FFmpeg scale filter
            // without explicitly breaking zero-copy using hwdownload. Thus if we try to scale while using D3D11, we MUST download.
            // When not scaling, skip hwdownload entirely.
            if out_w != width || out_h != height {
                warn!("Hardware encoding resolution scaling requested ({w}x{h} -> {out_w}x{out_h}). This breaks D3D11 zero-copy!", w=width, h=height, out_w=out_w, out_h=out_h);
                video_filters.push("hwdownload,format=bgra".to_string());
                video_filters.push(format!("scale={}:{}", out_w, out_h));
                // Add pixel format specifically since we downloaded it to bgra software format
                cmd.arg("-pix_fmt").arg("yuv420p");
            }
        }

        if !video_filters.is_empty() {
            cmd.arg("-vf").arg(video_filters.join(","));
        }
        cmd.arg("-c:v").arg(encoder_name);
        self.add_encoder_options(&mut cmd, encoder_name);
        let preset = self.preset_for_encoder(encoder_name);
        if !preset.is_empty() {
            cmd.arg("-preset").arg(preset);
        }
        if let Some(tune) = self.tune_for_encoder(encoder_name) {
            cmd.arg("-tune").arg(tune);
        }
        self.add_rate_control_options(&mut cmd, encoder_name);
        let output_format = if encoder_name.starts_with("h264_") {
            cmd.arg("-bsf:v").arg("h264_mp4toannexb");
            "h264"
        } else if encoder_name.starts_with("hevc_") {
            cmd.arg("-bsf:v").arg("hevc_mp4toannexb");
            "hevc"
        } else {
            "h264"
        };
        let keyframe_interval_secs =
            self.config.keyframe_interval_frames() as f64 / self.config.framerate.max(1) as f64;
        debug!(
            "Encoder keyframe policy: gop_frames={}, interval_secs={:.3}, force_key_frames=expr:gte(t,n_forced*{:.3})",
            self.config.keyframe_interval_frames(), keyframe_interval_secs,
            keyframe_interval_secs
        );
        cmd.arg("-g")
            .arg(self.config.keyframe_interval_frames().to_string())
            .arg("-force_key_frames")
            .arg(format!("expr:gte(t,n_forced*{keyframe_interval_secs:.3})"));

        // Explicitly set pixel format for CPU frames to avoid unplayable RGB streams,
        // but avoid it for D3D11 zero-copy which handles its own native formats (breaks auto-scale).
        // (If we had to download via hwdownload for scaling, the -pix_fmt is already appended above).
        if self.config.use_cpu_readback {
            cmd.arg("-pix_fmt").arg("yuv420p");
        }

        cmd.arg("-r")
            .arg(self.config.framerate.to_string())
            .arg("-fps_mode")
            .arg("cfr")
            .arg("-f")
            .arg(output_format)
            .arg("pipe:1");
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        if enabled!(Level::DEBUG) {
            let args: Vec<String> = std::iter::once(ffmpeg_cmd.clone())
                .chain(cmd.get_args().map(|s| s.to_string_lossy().to_string()))
                .collect();
            debug!("FFmpeg command: {}", args.join(" "));
        }
        let mut child = cmd
            .creation_flags(PROCESS_CREATION_FLAGS)
            .spawn()
            .with_context(|| format!("Failed to start FFmpeg ({})", encoder_name))?;
        let (stdin_for_process, async_writer) = if self.config.use_cpu_readback {
            debug!("FFmpeg stdin captured (CPU readback mode with async writer)");
            let stdin = child.stdin.take().context("Failed to take FFmpeg stdin")?;
            let writer = AsyncFrameWriter::new(stdin, 128); // Increased from 16 to reduce frame drops
            (None, Some(writer))
        } else {
            debug!("FFmpeg stdin not used (desktop-grab mode)");
            (None, None)
        };
        self.async_writer = async_writer;
        self.width = out_w;
        self.height = out_h;
        let (frame_meta_tx, frame_meta_rx) = bounded::<FrameMetadata>(32);
        self.frame_meta_tx = Some(frame_meta_tx);
        let packet_tx = self.packet_tx.clone();
        let stdout = child
            .stdout
            .take()
            .context("Failed to take FFmpeg stdout")?;
        let start_qpc = match query_qpc() {
            Ok(qpc) => qpc,
            Err(e) => {
                warn!("Failed to query QPC for encoder timeline start: {e:#}");
                0
            }
        };
        let stdout_handle = self.spawn_output_reader(stdout, packet_tx, frame_meta_rx, start_qpc);
        let stderr = child
            .stderr
            .take()
            .context("Failed to take FFmpeg stderr")?;
        let stderr_handle = self.spawn_stderr_reader(stderr);
        self.ffmpeg = Some(ManagedFfmpegProcess::new(
            child,
            stdin_for_process,
            stdout_handle,
            stderr_handle,
        ));
        info!(
            "Encoder ready: {} {}x{} @ {} FPS",
            encoder_name, width, height, self.config.framerate
        );
        Ok(())
    }
    /// Get preset for encoder type
    fn preset_for_encoder(&self, encoder_name: &str) -> &str {
        match encoder_name {
            "h264_nvenc" | "hevc_nvenc" | "av1_nvenc" => match self.config.quality_preset {
                QualityPreset::Performance => "p3",
                QualityPreset::Balanced => "p5",
                QualityPreset::Quality => "p7",
            },
            "h264_qsv" | "hevc_qsv" => match self.config.quality_preset {
                QualityPreset::Performance => "veryfast",
                QualityPreset::Balanced => "faster",
                QualityPreset::Quality => "medium",
            },
            "h264_amf" | "hevc_amf" | "av1_amf" => "",
            _ => match self.config.quality_preset {
                QualityPreset::Performance => "fast",
                QualityPreset::Balanced => "medium",
                QualityPreset::Quality => "slow",
            },
        }
    }
    /// Get tune parameter for encoder type
    fn tune_for_encoder(&self, encoder_name: &str) -> Option<&str> {
        match encoder_name {
            "h264_nvenc" | "hevc_nvenc" | "av1_nvenc" => {
                let tune = match self.config.quality_preset {
                    QualityPreset::Performance => "ull",
                    QualityPreset::Balanced => "ll",
                    QualityPreset::Quality => "hq",
                };
                Some(tune)
            }
            _ => None,
        }
    }
    /// Add encoder-specific options
    fn add_encoder_options(&self, cmd: &mut Command, encoder_name: &str) {
        match encoder_name {
            "h264_nvenc" | "hevc_nvenc" | "av1_nvenc" => {
                cmd.arg("-rc");
                cmd.arg(self.nvenc_rate_control_mode());
                if matches!(self.config.rate_control, RateControl::Cq) {
                    cmd.arg("-cq");
                    cmd.arg(self.cq_value().to_string());
                }
                cmd.arg("-delay");
                cmd.arg("0");
                cmd.arg("-b_ref_mode");
                cmd.arg("disabled");
                cmd.arg("-strict_gop");
                cmd.arg("1");
            }
            "h264_amf" | "hevc_amf" | "av1_amf" => {
                // Quality preset (speed/balanced/quality)
                cmd.arg("-quality");
                cmd.arg(self.amf_quality_mode());

                // CRITICAL: B-frames disabled for replay buffer compatibility
                // (Set to 2 or 3 if your replay buffer properly handles PTS/DTS sync for significant file size savings)
                cmd.arg("-bf").arg("0");

                // Usage mode: lowlatency is mandatory for real-time capture.
                // high_quality reserves maximum GPU resources for the encoder, causing
                // GPU contention with the game/desktop compositor and lagging the whole system.
                cmd.arg("-usage").arg("lowlatency");

                // === CODING EFFICIENCY ===
                // CABAC is ~10-15% more efficient than CAVLC for H.264
                if encoder_name == "h264_amf" {
                    cmd.arg("-coder").arg("cabac");
                }
                // HEVC profile tier - high tier allows better encoding tools
                if encoder_name == "hevc_amf" {
                    cmd.arg("-profile_tier").arg("high");
                }

                // === ADAPTIVE QUANTIZATION ===
                cmd.arg("-vbaq").arg("1"); // Variance-based AQ

                // === MOTION ESTIMATION ===
                cmd.arg("-me_half_pel").arg("1");
                cmd.arg("-me_quarter_pel").arg("1");
                // High motion quality boost - critical for gaming content
                cmd.arg("-high_motion_quality_boost_enable").arg("1");

                // === QP BOUNDS ===
                // Max limits relaxed (up to 51) to allow the rate control algorithm to breathe
                // and compress heavily during chaotic, unperceivable fast-motion scenes (shooters),
                // completely preventing gigantic bitrate spikes without hurting visible quality.
                cmd.arg("-min_qp_i").arg("18");
                cmd.arg("-max_qp_i").arg("51");
                cmd.arg("-min_qp_p").arg("20");
                cmd.arg("-max_qp_p").arg("51");

                // === STREAM STRUCTURE ===
                cmd.arg("-aud").arg("1"); // AU delimiter for clean seeking
                cmd.arg("-header_insertion_mode").arg("idr");
                cmd.arg("-gops_per_idr").arg("1");

                // === EFFICIENCY ===
                cmd.arg("-filler_data").arg("0"); // Don't waste bits on padding
            }
            "h264_qsv" | "hevc_qsv" => {
                cmd.arg("-look_ahead");
                cmd.arg("0");
                cmd.arg("-forced_idr");
                cmd.arg("1");
            }
            _ => {}
        }
    }
    /// Add generic bitrate/rate control options used by all encoder types.
    fn add_rate_control_options(&self, cmd: &mut Command, encoder_name: &str) {
        let bitrate_mbps = self.config.bitrate_mbps.max(1);
        let bitrate = format!("{}M", bitrate_mbps);

        // AMF rate control: use standard cbr/vbr modes.
        // hqcbr/hqvbr/qvbr are "high quality" variants that imply deeper buffering
        // and increased GPU resource reservation — not suitable for real-time capture.
        if matches!(encoder_name, "h264_amf" | "hevc_amf" | "av1_amf") {
            match self.config.rate_control {
                RateControl::Cbr => {
                    cmd.arg("-rc").arg("cbr");
                    cmd.arg("-b:v").arg(&bitrate);
                    cmd.arg("-maxrate").arg(&bitrate);
                    cmd.arg("-bufsize").arg(&bitrate);
                }
                RateControl::Vbr => {
                    let peak_mbps = bitrate_mbps.saturating_mul(2).max(1);
                    cmd.arg("-rc").arg("vbr_latency");
                    cmd.arg("-b:v").arg(&bitrate);
                    cmd.arg("-maxrate").arg(format!("{}M", peak_mbps));
                    cmd.arg("-bufsize").arg(&bitrate);
                }
                RateControl::Cq => {
                    // CQ maps to VBR with a quality target for AMF
                    let peak_mbps = bitrate_mbps.saturating_mul(2).max(1);
                    cmd.arg("-rc").arg("vbr_latency");
                    cmd.arg("-b:v").arg(&bitrate);
                    cmd.arg("-maxrate").arg(format!("{}M", peak_mbps));
                    cmd.arg("-bufsize").arg(&bitrate);
                }
            }
            return;
        }

        match self.config.rate_control {
            RateControl::Cbr => {
                let bufsize = format!("{}M", bitrate_mbps);
                cmd.arg("-b:v")
                    .arg(&bitrate)
                    .arg("-maxrate")
                    .arg(&bitrate)
                    .arg("-minrate")
                    .arg(&bitrate)
                    .arg("-bufsize")
                    .arg(bufsize);
            }
            RateControl::Vbr => {
                let peak_mbps = bitrate_mbps.saturating_mul(2).max(1);
                let peak = format!("{}M", peak_mbps);
                let bufsize = format!("{}M", bitrate_mbps);
                cmd.arg("-b:v")
                    .arg(&bitrate)
                    .arg("-maxrate")
                    .arg(&peak)
                    .arg("-bufsize")
                    .arg(bufsize);
            }
            RateControl::Cq => {
                let peak_mbps = bitrate_mbps.saturating_mul(2).max(1);
                let peak = format!("{}M", peak_mbps);
                let bufsize = format!("{}M", bitrate_mbps);
                let target_bitrate = if encoder_name.ends_with("_nvenc") {
                    "0".to_string()
                } else {
                    bitrate.clone()
                };
                cmd.arg("-b:v")
                    .arg(target_bitrate)
                    .arg("-maxrate")
                    .arg(&peak)
                    .arg("-bufsize")
                    .arg(bufsize);
            }
        }
    }
    fn amf_quality_mode(&self) -> &str {
        match self.config.quality_preset {
            QualityPreset::Performance => "speed",
            QualityPreset::Balanced => "balanced",
            QualityPreset::Quality => "quality",
        }
    }
    fn nvenc_rate_control_mode(&self) -> &str {
        match self.config.rate_control {
            RateControl::Cbr => "cbr",
            RateControl::Vbr => "vbr",
            RateControl::Cq => "vbr",
        }
    }
    fn cq_value(&self) -> u8 {
        self.config
            .quality_value
            .unwrap_or(match self.config.quality_preset {
                QualityPreset::Performance => 28,
                QualityPreset::Balanced => 25,
                QualityPreset::Quality => 22,
            })
    }
    /// Spawn thread to read FFmpeg output — returns JoinHandle for cleanup
    fn spawn_output_reader(
        &self,
        stdout: std::process::ChildStdout,
        packet_tx: Sender<EncodedPacket>,
        frame_meta_rx: Receiver<FrameMetadata>,
        start_qpc: i64,
    ) -> thread::JoinHandle<()> {
        let is_hevc = matches!(self.config.codec, Codec::H265);
        let expected_keyframe_interval_secs = (self.config.keyframe_interval_frames() as f64
            / self.config.framerate.max(1) as f64)
            .max(1.0);
        let min_frames_before_idr_warning = self.config.framerate.max(1) as u64;
        let qpc_freq = qpc_frequency();
        thread::spawn(move || {
            Self::set_encoder_thread_priority();
            debug!("FFmpeg output reader started");
            use std::cell::Cell;
            use std::io::Read;
            let mut reader = std::io::BufReader::new(stdout);
            let mut buffer = [0u8; 65536];
            let mut frame_buffer = BytesMut::with_capacity(1024 * 1024);
            let output_reader_start = Instant::now();
            let mut last_pts = start_qpc.saturating_sub(1);
            let mut avcc_nal_length_size = 4usize;
            let total_packets = Cell::new(0u64);
            let mut last_log_time = Instant::now();
            let mut bytes_received = 0u64;
            let frame_nals_seen = Cell::new(0u64);
            let idr_nals_seen = Cell::new(0u64);
            let last_idr_packet_count = Cell::new(0u64);
            let last_idr_wallclock_secs = Cell::new(0.0f64);
            let process_nal = |nal_data: bytes::Bytes,
                               is_last: bool,
                               last_pts: &mut i64,
                               frame_meta_rx: &Receiver<FrameMetadata>|
             -> bool {
                if nal_data.is_empty() {
                    return true;
                }
                let (nal_type, is_nonidr_intra, is_keyframe, is_frame_nal) = if is_hevc {
                    let nal_type = hevc_nal_type(&nal_data);
                    let is_frame_nal = matches!(nal_type, Some(0..=31));
                    let is_keyframe = matches!(nal_type, Some(16..=23));
                    (nal_type, false, is_keyframe, is_frame_nal)
                } else {
                    let nal_type = h264_nal_type(&nal_data);
                    let is_nonidr_intra =
                        matches!(nal_type, Some(1)) && h264_nonidr_is_intra_slice(&nal_data);
                    let is_keyframe = matches!(nal_type, Some(5)) || is_nonidr_intra;
                    let is_frame_nal = matches!(nal_type, Some(1 | 5));
                    (nal_type, is_nonidr_intra, is_keyframe, is_frame_nal)
                };
                if is_frame_nal {
                    frame_nals_seen.set(frame_nals_seen.get().saturating_add(1));
                }
                let count = total_packets.get() + 1;
                total_packets.set(count);
                if count == 1 || count % 600 == 0 || is_last || is_keyframe {
                    debug!(
                        "NAL packet {}: type={:?}, is_keyframe={}, is_frame_nal={}, size={} bytes",
                        count,
                        nal_type,
                        is_keyframe,
                        is_frame_nal,
                        nal_data.len()
                    );
                    if (!is_hevc && nal_type == Some(5))
                        || (is_hevc && matches!(nal_type, Some(16..=23)))
                    {
                        idr_nals_seen.set(idr_nals_seen.get().saturating_add(1));
                        last_idr_packet_count.set(count);
                        last_idr_wallclock_secs.set(output_reader_start.elapsed().as_secs_f64());
                        debug!(
                            "Detected keyframe NAL at packet {} - total_idr_nals={}, frame_nals_seen={}, codec={}",
                            count, idr_nals_seen.get(), frame_nals_seen.get(), if is_hevc
                            { "hevc" } else { "h264" }
                        );
                    } else if is_nonidr_intra {
                        debug!(
                            "Detected non-IDR intra slice at packet {} - treating as keyframe fallback",
                            count
                        );
                    }
                }
                let final_pts = if is_frame_nal {
                    match frame_meta_rx.try_recv() {
                        Ok(meta) => {
                            let pts = meta.capture_timestamp;
                            let normalized = if pts <= *last_pts { *last_pts + 1 } else { pts };
                            *last_pts = normalized;
                            normalized
                        }
                        Err(_) => {
                            let elapsed_qpc = (output_reader_start.elapsed().as_secs_f64()
                                * qpc_freq as f64)
                                as i64;
                            let pts = start_qpc.saturating_add(elapsed_qpc);
                            let normalized = if pts <= *last_pts { *last_pts + 1 } else { pts };
                            *last_pts = normalized;
                            normalized
                        }
                    }
                } else if *last_pts < start_qpc {
                    start_qpc
                } else {
                    *last_pts
                };
                let packet = EncodedPacket::new(
                    nal_data,
                    final_pts,
                    final_pts,
                    is_keyframe,
                    StreamType::Video,
                );
                if packet_tx.send(packet).is_err() {
                    debug!("FFmpeg output reader: channel closed");
                    return false;
                }
                true
            };
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => {
                        let count = total_packets.get();
                        debug!(
                            "FFmpeg output reader: EOF, flushing {} bytes as final NAL, total_packets={}",
                            frame_buffer.len(), count
                        );
                        if !frame_buffer.is_empty() {
                            let final_data = frame_buffer.split_to(frame_buffer.len()).freeze();
                            if !process_nal(final_data, true, &mut last_pts, &frame_meta_rx) {
                                return;
                            }
                        }
                        break;
                    }
                    Ok(n) => {
                        bytes_received += n as u64;
                        frame_buffer.extend_from_slice(&buffer[..n]);
                        if bytes_received == n as u64 {
                            debug!(
                                "FFmpeg output reader: received first {} bytes from stdout",
                                n
                            );
                        }
                        if last_log_time.elapsed() >= Duration::from_secs(15) {
                            let count = total_packets.get();
                            let now_secs = output_reader_start.elapsed().as_secs_f64();
                            let last_idr_packet = last_idr_packet_count.get();
                            let last_idr_secs = last_idr_wallclock_secs.get();
                            let since_last_idr_secs = if last_idr_packet == 0 {
                                now_secs
                            } else {
                                now_secs - last_idr_secs
                            };
                            debug!(
                                "FFmpeg output reader: {} bytes received, {} packets created, {} bytes in buffer, frame_nals={}, idr_nals={}, last_idr_packet={}, secs_since_last_idr={:.2}",
                                bytes_received, count, frame_buffer.len(), frame_nals_seen
                                .get(), idr_nals_seen.get(), last_idr_packet,
                                since_last_idr_secs
                            );
                            if frame_nals_seen.get() > min_frames_before_idr_warning
                                && since_last_idr_secs >= expected_keyframe_interval_secs * 3.0
                            {
                                warn!(
                                    "No IDR seen for {:.2}s (expected interval ~{:.2}s). Encoder may be emitting non-IDR I-frames only",
                                    since_last_idr_secs, expected_keyframe_interval_secs
                                );
                            }
                            last_log_time = Instant::now();
                        }
                        loop {
                            let Some((first_start, first_len)) =
                                find_annexb_start_code(&frame_buffer, 0)
                            else {
                                if frame_buffer.len() >= 7 && frame_buffer[0] == 1 {
                                    debug!(
                                        "Detected AVCC configuration record (avcC) header starting with 0x01"
                                    );
                                    let parsed_len_size = ((frame_buffer[4] & 0x03) as usize) + 1;
                                    if (1..=4).contains(&parsed_len_size) {
                                        avcc_nal_length_size = parsed_len_size;
                                    }
                                    let mut cursor = 6usize;
                                    let mut incomplete = false;
                                    let sps_count = (frame_buffer[5] & 0x1f) as usize;
                                    for _ in 0..sps_count {
                                        if frame_buffer.len() < cursor + 2 {
                                            incomplete = true;
                                            break;
                                        }
                                        let sps_len = u16::from_be_bytes([
                                            frame_buffer[cursor],
                                            frame_buffer[cursor + 1],
                                        ])
                                            as usize;
                                        cursor += 2;
                                        if frame_buffer.len() < cursor + sps_len {
                                            incomplete = true;
                                            break;
                                        }
                                        let mut annexb_nal = BytesMut::with_capacity(4 + sps_len);
                                        annexb_nal.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
                                        annexb_nal.extend_from_slice(
                                            &frame_buffer[cursor..cursor + sps_len],
                                        );
                                        if !process_nal(
                                            annexb_nal.freeze(),
                                            false,
                                            &mut last_pts,
                                            &frame_meta_rx,
                                        ) {
                                            return;
                                        }
                                        cursor += sps_len;
                                    }
                                    if incomplete {
                                        break;
                                    }
                                    if frame_buffer.len() < cursor + 1 {
                                        break;
                                    }
                                    let pps_count = frame_buffer[cursor] as usize;
                                    cursor += 1;
                                    for _ in 0..pps_count {
                                        if frame_buffer.len() < cursor + 2 {
                                            incomplete = true;
                                            break;
                                        }
                                        let pps_len = u16::from_be_bytes([
                                            frame_buffer[cursor],
                                            frame_buffer[cursor + 1],
                                        ])
                                            as usize;
                                        cursor += 2;
                                        if frame_buffer.len() < cursor + pps_len {
                                            incomplete = true;
                                            break;
                                        }
                                        let mut annexb_nal = BytesMut::with_capacity(4 + pps_len);
                                        annexb_nal.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
                                        annexb_nal.extend_from_slice(
                                            &frame_buffer[cursor..cursor + pps_len],
                                        );
                                        if !process_nal(
                                            annexb_nal.freeze(),
                                            false,
                                            &mut last_pts,
                                            &frame_meta_rx,
                                        ) {
                                            return;
                                        }
                                        cursor += pps_len;
                                    }
                                    if incomplete {
                                        break;
                                    }
                                    frame_buffer.advance(cursor);
                                    continue;
                                }
                                if frame_buffer.len() >= avcc_nal_length_size {
                                    if frame_buffer[0] == 0 && frame_buffer[1] == 0 {
                                        break;
                                    }
                                    let nal_len = match avcc_nal_length_size {
                                        1 => frame_buffer[0] as usize,
                                        2 => u16::from_be_bytes([frame_buffer[0], frame_buffer[1]])
                                            as usize,
                                        _ => u32::from_be_bytes([
                                            frame_buffer[0],
                                            frame_buffer[1],
                                            frame_buffer[2],
                                            frame_buffer[3],
                                        ]) as usize,
                                    };
                                    if nal_len == 0 {
                                        frame_buffer.advance(avcc_nal_length_size);
                                        continue;
                                    }
                                    if nal_len <= 10 * 1024 * 1024 {
                                        if frame_buffer.len() >= avcc_nal_length_size + nal_len {
                                            let mut avcc_nal = frame_buffer
                                                .split_to(avcc_nal_length_size + nal_len);
                                            let annexb_nal = if avcc_nal_length_size == 4 {
                                                avcc_nal[0..4]
                                                    .copy_from_slice(&[0x00, 0x00, 0x00, 0x01]);
                                                avcc_nal
                                            } else {
                                                let mut annexb_nal =
                                                    BytesMut::with_capacity(4 + nal_len);
                                                annexb_nal
                                                    .extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
                                                annexb_nal.extend_from_slice(
                                                    &avcc_nal[avcc_nal_length_size..],
                                                );
                                                annexb_nal
                                            };
                                            if !process_nal(
                                                annexb_nal.freeze(),
                                                false,
                                                &mut last_pts,
                                                &frame_meta_rx,
                                            ) {
                                                return;
                                            }
                                            continue;
                                        }
                                        break;
                                    }
                                }
                                if frame_buffer.len() > (5 * 1024 * 1024) {
                                    warn!(
                                        "Frame buffer exceeded 5MB without start code, discarding until next 00 00 01"
                                    );
                                    if let Some((pos, _)) = find_annexb_start_code(&frame_buffer, 1)
                                    {
                                        frame_buffer.advance(pos);
                                    } else {
                                        let keep = 4usize.min(frame_buffer.len());
                                        frame_buffer.advance(frame_buffer.len() - keep);
                                    }
                                }
                                break;
                            };
                            if first_start > 0 {
                                frame_buffer.advance(first_start);
                                continue;
                            }
                            let search_from = first_len;
                            let Some((second_start, _)) =
                                find_annexb_start_code(&frame_buffer, search_from)
                            else {
                                break;
                            };
                            let nal_data = frame_buffer.split_to(second_start).freeze();
                            if !process_nal(nal_data, false, &mut last_pts, &frame_meta_rx) {
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        error!("FFmpeg output reader error: {}", e);
                        break;
                    }
                }
            }
            let count = total_packets.get();
            info!(
                "Encoder output closed: {} bytes, {} packets, {} frame NALs",
                bytes_received,
                count,
                frame_nals_seen.get()
            );
        })
    }
    /// Spawn thread to read FFmpeg stderr for debugging — returns JoinHandle for cleanup
    fn spawn_stderr_reader(&self, stderr: std::process::ChildStderr) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            Self::set_encoder_thread_priority();
            debug!("FFmpeg stderr reader started");
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let lower = trimmed.to_ascii_lowercase();
                let is_error = lower.contains("error")
                    || lower.contains("failed")
                    || lower.contains("cannot")
                    || lower.contains("invalid");
                let is_progress = trimmed.starts_with("frame=");
                if is_error {
                    warn!("ffmpeg: {}", trimmed);
                } else if is_progress {
                    info!("ffmpeg progress: {}", trimmed);
                } else {
                    debug!("ffmpeg: {}", trimmed);
                }
            }
            debug!("FFmpeg stderr reader stopped");
        })
    }
    fn set_encoder_thread_priority() {
        #[cfg(windows)]
        {
            use windows::Win32::System::Threading::{
                GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_NORMAL,
            };
            unsafe {
                if let Err(e) = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_NORMAL) {
                    warn!("Failed to set encoder thread priority: {}", e);
                }
            }
        }
    }
    /// Encode a single frame
    pub(crate) fn encode_frame_internal(
        &mut self,
        frame: &crate::capture::CapturedFrame,
    ) -> Result<()> {
        if self.ffmpeg.is_none() {
            let (native_w, native_h) = frame.resolution;
            self.init_ffmpeg(native_w, native_h)?;
        }
        if let Some(ref meta_tx) = self.frame_meta_tx {
            let meta = FrameMetadata {
                capture_timestamp: frame.timestamp,
            };
            if meta_tx.try_send(meta).is_err() && self.frame_count % 60 == 0 {
                debug!(
                    "Frame metadata channel full, dropping timestamp for frame {}",
                    self.frame_count
                );
            }
        }
        if !self.config.use_cpu_readback {
            self.frame_count += 1;
            return Ok(());
        }
        if let Some(ref async_writer) = self.async_writer {
            use crate::encode::frame_writer::PendingFrame;
            use crossbeam::channel::TrySendError;
            let pending = PendingFrame {
                data: frame.bgra.clone(),
                timestamp: frame.timestamp,
            };
            match async_writer.try_queue(pending) {
                Ok(()) => {
                    self.frame_count += 1;
                    if self.frame_count == 1 || self.frame_count % 600 == 0 {
                        debug!(
                            "Queued frame {} for async write ({} bytes)",
                            self.frame_count,
                            frame.bgra.len()
                        );
                    }
                }
                Err(TrySendError::Full(_)) => {
                    self.dropped_frames += 1;
                    self.frame_count += 1;
                    let queue_len = async_writer.queue_len();
                    let queue_cap = async_writer.queue_capacity();
                    let latency = async_writer.write_latency_ms();

                    // Log first drop and then every 500 drops with diagnostics
                    if self.dropped_frames == 1 || self.dropped_frames % 500 == 0 {
                        warn!(
                            "Dropped {} frames (total). Queue: {}/{} ({}%), write latency: {}ms.                              FFmpeg stdin write is slower than capture rate - consider lowering FPS or resolution",
                            self.dropped_frames,
                            queue_len,
                            queue_cap,
                            (queue_len * 100) / queue_cap.max(1),
                            latency
                        );
                    }
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.dropped_frames += 1;
                    self.frame_count += 1;
                    warn!(
                        "Async writer channel disconnected, dropped frame {} (total {} dropped)",
                        self.frame_count, self.dropped_frames
                    );
                }
            }
        } else {
            warn!("No async writer available for FFmpeg");
        }
        Ok(())
    }
    /// Flush remaining frames and close FFmpeg
    pub(crate) fn flush_internal(&mut self) -> Result<Vec<EncodedPacket>> {
        if self.ffmpeg.take().is_some() {
            debug!("FFmpeg process cleaned up via ManagedFfmpegProcess::Drop");
        }
        if self.async_writer.take().is_some() {
            debug!("Async frame writer stopped");
        }
        self.running = false;
        let mut packets = Vec::new();
        while let Ok(packet) = self.packet_rx.try_recv() {
            packets.push(packet);
        }
        info!("Flushed {} packets from hardware encoder", packets.len());
        Ok(packets)
    }
}
/// Frame metadata passed from encoder thread to output reader thread.
/// Used to preserve original capture timestamps for A/V sync.
struct FrameMetadata {
    /// Original QPC timestamp from capture (10MHz units)
    capture_timestamp: i64,
}
/// NVENC encoder wrapper (NVIDIA)
pub struct NvencEncoder {
    pub(super) base: HardwareEncoderBase,
}
impl NvencEncoder {
    pub fn new(config: &EncoderConfig) -> Result<Self> {
        debug!("Creating NVENC encoder");
        let encoder_name = match config.codec {
            Codec::H264 => "h264_nvenc",
            Codec::H265 => "hevc_nvenc",
            Codec::Av1 => "av1_nvenc",
        };
        let base = HardwareEncoderBase::new(config, encoder_name)?;
        Ok(Self { base })
    }
}
