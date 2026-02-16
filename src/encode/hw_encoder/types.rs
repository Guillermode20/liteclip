//! Auto-generated module
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use crate::encode::{EncodedPacket, Encoder, EncoderConfig, StreamType};
use crate::config::{QualityPreset, RateControl};
use anyhow::{Context, Result};
use bytes::{Buf, BytesMut};
use crossbeam::channel::{bounded, Receiver, Sender};
use std::io::{BufRead, BufReader, Write};
use std::process::{ChildStdin, Command, Stdio};
use std::thread;
use std::time::Instant;
use tracing::{debug, enabled, error, info, trace, warn, Level};

use super::functions::{find_annexb_start_code, h264_nal_type, query_qpc, resolve_ffmpeg_command};
use crate::buffer::ring::qpc_frequency;


/// Base hardware encoder using FFmpeg CLI
pub struct HardwareEncoderBase {
    pub(crate) config: EncoderConfig,
    encoder_name: String,
    pub(crate) packet_rx: Receiver<EncodedPacket>,
    pub(crate) packet_tx: Sender<EncodedPacket>,
    frame_count: u64,
    pub(crate) running: bool,
    pub(super) ffmpeg_process: Option<std::process::Child>,
    pub(super) ffmpeg_stdin: Option<ChildStdin>,
    width: u32,
    height: u32,
    /// JoinHandle for the stdout reader thread
    pub(super) stdout_thread: Option<thread::JoinHandle<()>>,
    /// JoinHandle for the stderr reader thread
    pub(super) stderr_thread: Option<thread::JoinHandle<()>>,
    /// Channel sender for frame metadata (timestamps) to output reader
    frame_meta_tx: Option<Sender<FrameMetadata>>,
}
impl HardwareEncoderBase {
    /// Create new hardware encoder with FFmpeg CLI
    pub fn new(config: &EncoderConfig, encoder_name: &str) -> Result<Self> {
        let ffmpeg_cmd = resolve_ffmpeg_command();
        info!("Creating {} encoder with FFmpeg: {}", encoder_name, ffmpeg_cmd);
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
            ffmpeg_process: None,
            ffmpeg_stdin: None,
            width,
            height,
            stdout_thread: None,
            stderr_thread: None,
            frame_meta_tx: None,
        })
    }
    /// Initialize the FFmpeg process with hardware encoder settings
    pub(crate) fn init_ffmpeg(&mut self, width: u32, height: u32) -> Result<()> {
        let ffmpeg_cmd = resolve_ffmpeg_command();
        let encoder_name = self.encoder_name.as_str();
        let (out_w, out_h) = if !self.config.use_native_resolution
            && self.config.resolution.0 > 0 && self.config.resolution.1 > 0
        {
            self.config.resolution
        } else {
            (width, height)
        };
        let mut cmd = Command::new(&ffmpeg_cmd);
        cmd.arg("-y");
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
                .arg("pipe:0")
                .arg("-vsync")
                .arg("cfr");
        } else {
            cmd.arg("-f")
                .arg("lavfi")
                .arg("-i")
                .arg(
                    format!(
                        "ddagrab=output_idx={}:framerate={}", self.config.output_index,
                        self.config.framerate
                    ),
                )
                .arg("-vsync")
                .arg("cfr");
            if matches!(encoder_name, "h264_amf" | "hevc_amf" | "av1_amf") {
                video_filters.push("hwdownload,format=bgra".to_string());
            }
        }
        if out_w != width || out_h != height {
            video_filters.push(format!("scale={}:{}", out_w, out_h));
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
        cmd.arg("-g")
            .arg(self.config.keyframe_interval_frames().to_string())
            .arg("-pix_fmt")
            .arg("yuv420p")
            .arg("-r")
            .arg(self.config.framerate.to_string())
            .arg("-vsync")
            .arg("cfr")
            .arg("-f")
            .arg("h264")
            .arg("pipe:1");
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        if enabled!(Level::INFO) {
            let args: Vec<String> = std::iter::once(ffmpeg_cmd.clone())
                .chain(cmd.get_args().map(|s| s.to_string_lossy().to_string()))
                .collect();
            info!("FFmpeg command: {}", args.join(" "));
        }
        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to start FFmpeg ({})", encoder_name))?;
        let stdin = if self.config.use_cpu_readback {
            Some(child.stdin.take().context("Failed to take FFmpeg stdin")?)
        } else {
            None
        };
        self.ffmpeg_process = Some(child);
        self.ffmpeg_stdin = stdin;
        self.width = out_w;
        self.height = out_h;
        let (frame_meta_tx, frame_meta_rx) = bounded::<FrameMetadata>(256);
        self.frame_meta_tx = Some(frame_meta_tx);
        let packet_tx = self.packet_tx.clone();
        let stdout = self
            .ffmpeg_process
            .as_mut()
            .unwrap()
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
        let stdout_handle = self
            .spawn_output_reader(stdout, packet_tx, frame_meta_rx, start_qpc);
        self.stdout_thread = Some(stdout_handle);
        let stderr = self
            .ffmpeg_process
            .as_mut()
            .unwrap()
            .stderr
            .take()
            .context("Failed to take FFmpeg stderr")?;
        let stderr_handle = self.spawn_stderr_reader(stderr);
        self.stderr_thread = Some(stderr_handle);
        info!(
            "FFmpeg {} encoder initialized: {}x{} @ {} FPS", encoder_name, width, height,
            self.config.framerate
        );
        Ok(())
    }
    /// Get preset for encoder type
    fn preset_for_encoder(&self, encoder_name: &str) -> &str {
        match encoder_name {
            "h264_nvenc" | "hevc_nvenc" | "av1_nvenc" => {
                match self.config.quality_preset {
                    QualityPreset::Performance => "p3",
                    QualityPreset::Balanced => "p5",
                    QualityPreset::Quality => "p7",
                }
            }
            "h264_qsv" | "hevc_qsv" => {
                match self.config.quality_preset {
                    QualityPreset::Performance => "veryfast",
                    QualityPreset::Balanced => "faster",
                    QualityPreset::Quality => "medium",
                }
            }
            "h264_amf" | "hevc_amf" | "av1_amf" => "",
            _ => {
                match self.config.quality_preset {
                    QualityPreset::Performance => "fast",
                    QualityPreset::Balanced => "medium",
                    QualityPreset::Quality => "slow",
                }
            }
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
                cmd.arg("-tune");
                cmd.arg("ull");
                cmd.arg("-b_ref_mode");
                cmd.arg("disabled");
            }
            "h264_amf" | "hevc_amf" | "av1_amf" => {
                cmd.arg("-quality");
                cmd.arg(self.amf_quality_mode());
                cmd.arg("-bf");
                cmd.arg("0");
                cmd.arg("-sei");
                cmd.arg("+aud");
                cmd.arg("-vsync");
                cmd.arg("cfr");
                cmd.arg("-usage");
                cmd.arg("lowlatency");
            }
            "h264_qsv" | "hevc_qsv" => {
                cmd.arg("-look_ahead");
                cmd.arg("0");
                cmd.arg("-preset");
                cmd.arg("faster");
            }
            _ => {}
        }
    }
    /// Add generic bitrate/rate control options used by all encoder types.
    fn add_rate_control_options(&self, cmd: &mut Command, encoder_name: &str) {
        let bitrate_mbps = self.config.bitrate_mbps.max(1);
        let bitrate = format!("{}M", bitrate_mbps);
        match self.config.rate_control {
            RateControl::Cbr => {
                let bufsize = format!("{}M", bitrate_mbps.saturating_mul(2).max(1));
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
                cmd.arg("-b:v")
                    .arg(&bitrate)
                    .arg("-maxrate")
                    .arg(&peak)
                    .arg("-bufsize")
                    .arg(peak);
            }
            RateControl::Cq => {
                let peak_mbps = bitrate_mbps.saturating_mul(2).max(1);
                let peak = format!("{}M", peak_mbps);
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
                    .arg(peak);
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
            .unwrap_or(
                match self.config.quality_preset {
                    QualityPreset::Performance => 28,
                    QualityPreset::Balanced => 23,
                    QualityPreset::Quality => 19,
                },
            )
    }
    /// Spawn thread to read FFmpeg output — returns JoinHandle for cleanup
    fn spawn_output_reader(
        &self,
        stdout: std::process::ChildStdout,
        packet_tx: Sender<EncodedPacket>,
        frame_meta_rx: Receiver<FrameMetadata>,
        start_qpc: i64,
    ) -> thread::JoinHandle<()> {
        let qpc_freq = qpc_frequency();
        thread::spawn(move || {
            use std::io::Read;
            let mut reader = std::io::BufReader::new(stdout);
            let mut buffer = [0u8; 65536];
            let mut frame_buffer = BytesMut::with_capacity(1024 * 1024);
            let output_reader_start = Instant::now();
            let mut last_pts = start_qpc.saturating_sub(1);
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => {
                        debug!("FFmpeg output reader: EOF");
                        break;
                    }
                    Ok(n) => {
                        frame_buffer.extend_from_slice(&buffer[..n]);
                        loop {
                            let Some((start_pos, start_len)) = find_annexb_start_code(
                                &frame_buffer,
                                0,
                            ) else {
                                if frame_buffer.len() > (2 * 1024 * 1024) {
                                    let keep = 4usize.min(frame_buffer.len());
                                    frame_buffer.advance(frame_buffer.len() - keep);
                                }
                                break;
                            };
                            if start_pos > 0 {
                                frame_buffer.advance(start_pos);
                                continue;
                            }
                            let Some((next_start, _)) = find_annexb_start_code(
                                &frame_buffer,
                                start_len,
                            ) else {
                                break;
                            };
                            let nal_data = frame_buffer.split_to(next_start).freeze();
                            if !nal_data.is_empty() {
                                let nal_type = h264_nal_type(&nal_data);
                                let is_keyframe = matches!(nal_type, Some(5 | 7 | 8));
                                let is_frame_nal = matches!(nal_type, Some(1 | 5));
                                let final_pts = if is_frame_nal {
                                    match frame_meta_rx.try_recv() {
                                        Ok(meta) => {
                                            let pts = meta.capture_timestamp;
                                            let normalized = if pts <= last_pts {
                                                last_pts + 1
                                            } else {
                                                pts
                                            };
                                            last_pts = normalized;
                                            normalized
                                        }
                                        Err(_) => {
                                            let elapsed_qpc = (output_reader_start
                                                .elapsed()
                                                .as_secs_f64() * qpc_freq as f64) as i64;
                                            let pts = start_qpc.saturating_add(elapsed_qpc);
                                            let normalized = if pts <= last_pts {
                                                last_pts + 1
                                            } else {
                                                pts
                                            };
                                            last_pts = normalized;
                                            normalized
                                        }
                                    }
                                } else if last_pts < start_qpc {
                                    start_qpc
                                } else {
                                    last_pts
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
                                    return;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        error!("FFmpeg output reader error: {}", e);
                        break;
                    }
                }
            }
            debug!("FFmpeg output reader thread exiting");
        })
    }
    /// Spawn thread to read FFmpeg stderr for debugging — returns JoinHandle for cleanup
    fn spawn_stderr_reader(
        &self,
        stderr: std::process::ChildStderr,
    ) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                debug!("[FFmpeg] {}", line);
            }
            debug!("FFmpeg stderr reader thread exiting");
        })
    }
    /// Encode a single frame
    pub(crate) fn encode_frame_internal(
        &mut self,
        frame: &crate::capture::CapturedFrame,
    ) -> Result<()> {
        if self.ffmpeg_process.is_none() {
            let (native_w, native_h) = frame.resolution;
            self.init_ffmpeg(native_w, native_h)?;
        }
        if let Some(ref meta_tx) = self.frame_meta_tx {
            let meta = FrameMetadata {
                capture_timestamp: frame.timestamp,
            };
            if meta_tx.try_send(meta).is_err() && self.frame_count % 60 == 0 {
                debug!(
                    "Frame metadata channel full, dropping timestamp for frame {}", self
                    .frame_count
                );
            }
        }
        if !self.config.use_cpu_readback {
            self.frame_count += 1;
            return Ok(());
        }
        if let Some(ref mut stdin) = self.ffmpeg_stdin {
            let data = &frame.bgra;
            stdin.write_all(data).context("Failed to write frame to FFmpeg stdin")?;
            self.frame_count += 1;
            if self.frame_count % 60 == 0 {
                trace!("Encoded frame {} to FFmpeg", self.frame_count);
            }
        }
        Ok(())
    }
    /// Flush remaining frames and close FFmpeg
    pub(crate) fn flush_internal(&mut self) -> Result<Vec<EncodedPacket>> {
        if let Some(stdin) = self.ffmpeg_stdin.take() {
            drop(stdin);
        }
        if let Some(mut child) = self.ffmpeg_process.take() {
            let timeout = std::time::Duration::from_secs(10);
            let start = Instant::now();
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        if !status.success() {
                            warn!("FFmpeg process exited with status: {}", status);
                        }
                        break;
                    }
                    Ok(None) => {
                        if start.elapsed() > timeout {
                            warn!(
                                "FFmpeg process did not exit within {:?}, killing", timeout
                            );
                            let _ = child.kill();
                            let _ = child.wait();
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                    Err(e) => {
                        warn!("Error waiting for FFmpeg process: {}", e);
                        break;
                    }
                }
            }
        }
        let join_timeout = std::time::Duration::from_secs(5);
        if let Some(handle) = self.stdout_thread.take() {
            let start = Instant::now();
            while !handle.is_finished() {
                if start.elapsed() > join_timeout {
                    warn!(
                        "stdout reader thread did not finish within {:?}", join_timeout
                    );
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            if handle.is_finished() {
                let _ = handle.join();
            }
        }
        if let Some(handle) = self.stderr_thread.take() {
            let start = Instant::now();
            while !handle.is_finished() {
                if start.elapsed() > join_timeout {
                    warn!(
                        "stderr reader thread did not finish within {:?}", join_timeout
                    );
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            if handle.is_finished() {
                let _ = handle.join();
            }
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
        let base = HardwareEncoderBase::new(config, "h264_nvenc")?;
        Ok(Self { base })
    }
}
/// QSV encoder wrapper (Intel)
pub struct QsvEncoder {
    pub(super) base: HardwareEncoderBase,
}
impl QsvEncoder {
    pub fn new(config: &EncoderConfig) -> Result<Self> {
        debug!("Creating QSV encoder");
        let base = HardwareEncoderBase::new(config, "h264_qsv")?;
        Ok(Self { base })
    }
}
/// AMF encoder wrapper (AMD)
pub struct AmfEncoder {
    pub(super) base: HardwareEncoderBase,
}
impl AmfEncoder {
    pub fn new(config: &EncoderConfig) -> Result<Self> {
        debug!("Creating AMF encoder");
        let base = HardwareEncoderBase::new(config, "h264_amf")?;
        Ok(Self { base })
    }
}
