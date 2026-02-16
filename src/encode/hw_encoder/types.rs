//! Auto-generated module
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use crate::config::{QualityPreset, RateControl};
use crate::encode::{EncodedPacket, EncoderConfig, StreamType};
use anyhow::{Context, Result};
use bytes::{Buf, BytesMut};
use crossbeam::channel::{bounded, Receiver, Sender};
use std::io::{BufRead, BufReader, Write};
use std::process::{ChildStdin, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tracing::{debug, enabled, error, info, warn, Level};

use super::functions::{find_annexb_start_code, h264_nal_type, query_qpc, resolve_ffmpeg_command};
use crate::buffer::ring::qpc_frequency;

/// Frame metadata passed from encoder thread to output reader thread.
/// Used to preserve original capture timestamps for A/V sync.
struct FrameMetadata {
    /// Original QPC timestamp from capture (10MHz units)
    capture_timestamp: i64,
}

/// Managed FFmpeg process handle that ensures proper cleanup on drop.
/// Encapsulates the child process, stdin, and reader threads.
pub struct ManagedFfmpegProcess {
    /// The FFmpeg child process
    child: std::process::Child,
    /// Stdin handle (only present in CPU readback mode)
    stdin: Option<ChildStdin>,
    /// Handle for the stdout reader thread (Option allows take() in Drop)
    stdout_reader: Option<thread::JoinHandle<()>>,
    /// Handle for the stderr reader thread (Option allows take() in Drop)
    stderr_reader: Option<thread::JoinHandle<()>>,
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

impl Drop for ManagedFfmpegProcess {
    fn drop(&mut self) {
        // Drop stdin first to signal EOF to FFmpeg
        if let Some(stdin) = self.stdin.take() {
            drop(stdin);
        }

        // Wait for the process to exit with timeout
        let process_timeout = Duration::from_secs(10);
        let start = Instant::now();
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        warn!("FFmpeg process exited with status: {}", status);
                    }
                    break;
                }
                Ok(None) => {
                    if start.elapsed() > process_timeout {
                        warn!(
                            "FFmpeg process did not exit within {:?}, killing",
                            process_timeout
                        );
                        let _ = self.child.kill();
                        let _ = self.child.wait();
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    warn!("Error waiting for FFmpeg process: {}", e);
                    break;
                }
            }
        }

        // Wait for reader threads with timeout
        let thread_timeout = Duration::from_secs(5);

        // Handle stdout reader - take ownership to allow join()
        if let Some(stdout_reader) = self.stdout_reader.take() {
            let start = Instant::now();
            while !stdout_reader.is_finished() {
                if start.elapsed() > thread_timeout {
                    warn!(
                        "stdout reader thread did not finish within {:?}",
                        thread_timeout
                    );
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            if stdout_reader.is_finished() {
                let _ = stdout_reader.join();
            }
        }

        // Handle stderr reader - take ownership to allow join()
        if let Some(stderr_reader) = self.stderr_reader.take() {
            let start = Instant::now();
            while !stderr_reader.is_finished() {
                if start.elapsed() > thread_timeout {
                    warn!(
                        "stderr reader thread did not finish within {:?}",
                        thread_timeout
                    );
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            if stderr_reader.is_finished() {
                let _ = stderr_reader.join();
            }
        }
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
        } else {
            cmd.arg("-f").arg("lavfi").arg("-i").arg(format!(
                "ddagrab=output_idx={}:framerate={}",
                self.config.output_index, self.config.framerate
            ));
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
        if encoder_name.starts_with("h264_") {
            cmd.arg("-bsf:v").arg("h264_mp4toannexb");
        }
        cmd.arg("-g")
            .arg(self.config.keyframe_interval_frames().to_string())
            .arg("-pix_fmt")
            .arg("yuv420p")
            .arg("-r")
            .arg(self.config.framerate.to_string())
            .arg("-fps_mode")
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
        info!("Spawning FFmpeg process...");
        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to start FFmpeg ({})", encoder_name))?;
        info!("FFmpeg process spawned successfully");
        let stdin = if self.config.use_cpu_readback {
            info!("Capturing FFmpeg stdin for CPU readback mode");
            Some(child.stdin.take().context("Failed to take FFmpeg stdin")?)
        } else {
            info!("Skipping stdin capture (ddagrab mode)");
            None
        };
        self.width = out_w;
        self.height = out_h;
        let (frame_meta_tx, frame_meta_rx) = bounded::<FrameMetadata>(256);
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

        // Create the managed process
        self.ffmpeg = Some(ManagedFfmpegProcess::new(
            child,
            stdin,
            stdout_handle,
            stderr_handle,
        ));

        info!(
            "FFmpeg {} encoder initialized: {}x{} @ {} FPS",
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
                cmd.arg("-usage");
                cmd.arg("lowlatency");
            }
            "h264_qsv" | "hevc_qsv" => {
                cmd.arg("-look_ahead");
                cmd.arg("0");
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
                // Use a smaller buffer (1s) to avoid stall/latency
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
                QualityPreset::Balanced => 23,
                QualityPreset::Quality => 19,
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
        let qpc_freq = qpc_frequency();
        thread::spawn(move || {
            info!("FFmpeg output reader thread started");
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

            // Helper function to process a NAL unit
            let process_nal = |nal_data: bytes::Bytes,
                               is_last: bool,
                               last_pts: &mut i64,
                               frame_meta_rx: &Receiver<FrameMetadata>|
             -> bool {
                if nal_data.is_empty() {
                    return true;
                }
                let nal_type = h264_nal_type(&nal_data);
                let is_keyframe = matches!(nal_type, Some(5 | 7 | 8));
                let is_frame_nal = matches!(nal_type, Some(1 | 5));

                let count = total_packets.get() + 1;
                total_packets.set(count);
                if count == 1 || count % 600 == 0 || is_last {
                    debug!(
                        "NAL packet {}: type={:?}, is_keyframe={}, is_frame_nal={}, size={} bytes",
                        count,
                        nal_type,
                        is_keyframe,
                        is_frame_nal,
                        nal_data.len()
                    );
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
                        // EOF - process any remaining data as final NAL
                        let count = total_packets.get();
                        debug!("FFmpeg output reader: EOF, flushing {} bytes as final NAL, total_packets={}",
                            frame_buffer.len(), count);
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

                        // Log first bytes received
                        if bytes_received == n as u64 {
                            debug!(
                                "FFmpeg output reader: received first {} bytes from stdout",
                                n
                            );
                        }

                        // Periodic logging
                        if last_log_time.elapsed() >= Duration::from_secs(10) {
                            let count = total_packets.get();
                            debug!(
                                "FFmpeg output reader: {} bytes received, {} packets created, {} bytes in buffer",
                                bytes_received,
                                count,
                                frame_buffer.len()
                            );
                            last_log_time = Instant::now();
                        }

                        // Process all complete NAL units in the buffer
                        loop {
                            // Find first start code
                            let Some((first_start, first_len)) =
                                find_annexb_start_code(&frame_buffer, 0)
                            else {
                                // No start code found in buffer

                                // Handle AVCDecoderConfigurationRecord (avcC) if present.
                                // Some encoder paths emit this once before length-prefixed NAL units.
                                if frame_buffer.len() >= 7 && frame_buffer[0] == 1 {
                                    debug!("Detected AVCC configuration record (avcC) header starting with 0x01");
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

                                // Fallback: some encoder paths can emit length-prefixed (AVCC) NALs.
                                // Convert one AVCC NAL to Annex-B and emit it when possible.
                                if frame_buffer.len() >= avcc_nal_length_size {
                                    // ... existing logic but skip if it might be Annex-B start code
                                    if frame_buffer[0] == 0 && frame_buffer[1] == 0 {
                                        // Wait for more data to see if it's Annex-B
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
                                        // Allow up to 10MB frames
                                        if frame_buffer.len() >= avcc_nal_length_size + nal_len {
                                            let avcc_nal = frame_buffer
                                                .split_to(avcc_nal_length_size + nal_len);
                                            let mut annexb_nal =
                                                BytesMut::with_capacity(4 + nal_len);
                                            annexb_nal.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
                                            annexb_nal.extend_from_slice(
                                                &avcc_nal[avcc_nal_length_size..],
                                            );
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

                                        // Incomplete AVCC payload - wait for more data.
                                        break;
                                    }
                                }

                                // No start code found - if buffer is too large, search for ANY 00 00 01
                                if frame_buffer.len() > (5 * 1024 * 1024) {
                                    warn!("Frame buffer exceeded 5MB without start code, discarding until next 00 00 01");
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

                            // Skip any garbage before the first start code
                            if first_start > 0 {
                                frame_buffer.advance(first_start);
                                continue;
                            }

                            // Find second start code (marks end of first NAL)
                            let search_from = first_len;
                            let Some((second_start, _)) =
                                find_annexb_start_code(&frame_buffer, search_from)
                            else {
                                // No second start code - wait for more data to ensure we have the full NAL
                                break;
                            };

                            // Extract the complete NAL unit
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
                "FFmpeg output reader thread exiting: {} bytes received, {} packets created",
                bytes_received, count
            );
        })
    }
    /// Spawn thread to read FFmpeg stderr for debugging — returns JoinHandle for cleanup
    fn spawn_stderr_reader(&self, stderr: std::process::ChildStderr) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            info!("FFmpeg stderr reader thread started");
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                info!("[FFmpeg stderr] {}", line);
            }
            info!("FFmpeg stderr reader thread exiting");
        })
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
        if let Some(ref mut ffmpeg) = self.ffmpeg {
            if let Some(stdin) = ffmpeg.stdin_mut() {
                let data = &frame.bgra;
                stdin
                    .write_all(data)
                    .context("Failed to write frame to FFmpeg stdin")?;
                self.frame_count += 1;
                if self.frame_count <= 10 || self.frame_count % 60 == 0 {
                    info!(
                        "Sent frame {} to FFmpeg ({} bytes)",
                        self.frame_count,
                        data.len()
                    );
                }
            } else {
                warn!("No stdin available for FFmpeg");
            }
        } else {
            warn!("FFmpeg process not initialized");
        }
        Ok(())
    }
    /// Flush remaining frames and close FFmpeg
    pub(crate) fn flush_internal(&mut self) -> Result<Vec<EncodedPacket>> {
        // Drop the managed process - this triggers ManagedFfmpegProcess::Drop
        // which handles all cleanup (stdin close, process wait with timeout,
        // thread joins with timeouts)
        if self.ffmpeg.take().is_some() {
            debug!("FFmpeg process cleaned up via ManagedFfmpegProcess::Drop");
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
