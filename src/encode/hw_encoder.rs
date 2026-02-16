//! Hardware Encoder Implementations (NVENC, AMF, QSV)
//!
//! Uses FFmpeg CLI with hardware-accelerated encoding (h264_nvenc, h264_amf, h264_qsv).
//! Each encoder spawns an FFmpeg child process that receives BGRA frames and outputs
//! H.264 NAL units directly, avoiding the MJPEG intermediate step.

use super::{EncodedPacket, Encoder, EncoderConfig, StreamType};
use crate::config::{QualityPreset, RateControl};
use anyhow::{Context, Result};
use bytes::{Buf, BytesMut};
use crossbeam::channel::{bounded, Receiver, Sender};
use std::io::{BufRead, BufReader, Write};
use std::process::{ChildStdin, Command, Stdio};
use std::thread;
use std::time::Instant;
use tracing::{debug, enabled, error, info, trace, warn, Level};

// Import the QPC frequency function from the buffer module
use crate::buffer::ring::qpc_frequency;

/// Frame metadata passed from encoder thread to output reader thread.
/// Used to preserve original capture timestamps for A/V sync.
struct FrameMetadata {
    /// Original QPC timestamp from capture (10MHz units)
    capture_timestamp: i64,
}

/// Convert BGRA to RGB24 format for FFmpeg input
#[cfg(test)]
fn bgra_to_rgb24(bgra: &[u8], width: u32, height: u32) -> Vec<u8> {
    let expected_len = (width * height * 4) as usize;
    if bgra.len() != expected_len {
        return vec![];
    }

    let pixel_count = (width * height) as usize;
    let mut rgb = Vec::with_capacity(pixel_count * 3);

    // SAFETY: We pre-allocated enough capacity for pixel_count * 3 bytes.
    // We write exactly pixel_count * 3 bytes via raw pointer, then set_len.
    // The BGRA slice is validated to have exactly pixel_count * 4 bytes.
    unsafe {
        let mut dst: *mut u8 = rgb.as_mut_ptr();
        let src: *const u8 = bgra.as_ptr();

        for i in 0..pixel_count {
            let src_offset = i * 4;
            // BGRA order: [B, G, R, A] -> RGB order: [R, G, B]
            dst.write(*src.add(src_offset + 2)); // R
            dst = dst.add(1);
            dst.write(*src.add(src_offset + 1)); // G
            dst = dst.add(1);
            dst.write(*src.add(src_offset)); // B
            dst = dst.add(1);
        }

        rgb.set_len(pixel_count * 3);
    }

    rgb
}

fn find_annexb_start_code(data: &[u8], from: usize) -> Option<(usize, usize)> {
    if data.len() < 3 || from >= data.len() {
        return None;
    }

    let mut i = from;
    while i + 2 < data.len() {
        if i + 3 < data.len()
            && data[i] == 0x00
            && data[i + 1] == 0x00
            && data[i + 2] == 0x00
            && data[i + 3] == 0x01
        {
            return Some((i, 4));
        }

        if data[i] == 0x00 && data[i + 1] == 0x00 && data[i + 2] == 0x01 {
            return Some((i, 3));
        }

        i += 1;
    }

    None
}

fn h264_nal_type(nal_data: &[u8]) -> Option<u8> {
    if nal_data.len() >= 5
        && nal_data[0] == 0x00
        && nal_data[1] == 0x00
        && nal_data[2] == 0x00
        && nal_data[3] == 0x01
    {
        return Some(nal_data[4] & 0x1f);
    }

    if nal_data.len() >= 4 && nal_data[0] == 0x00 && nal_data[1] == 0x00 && nal_data[2] == 0x01 {
        return Some(nal_data[3] & 0x1f);
    }

    None
}

/// Resolve FFmpeg command path
fn resolve_ffmpeg_command() -> String {
    // Check environment variable first
    if let Ok(custom) = std::env::var("LITECLIP_FFMPEG_PATH") {
        if !custom.trim().is_empty() {
            return custom;
        }
    }

    // Check local ffmpeg directory
    if let Ok(cwd) = std::env::current_dir() {
        let candidate = cwd.join("ffmpeg").join("bin").join("ffmpeg.exe");
        if candidate.exists() {
            return candidate.to_string_lossy().to_string();
        }
    }

    // Check alongside executable
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let candidate = exe_dir.join("ffmpeg").join("bin").join("ffmpeg.exe");
            if candidate.exists() {
                return candidate.to_string_lossy().to_string();
            }
        }
    }

    // Fallback to system ffmpeg
    "ffmpeg".to_string()
}

fn query_qpc() -> Result<i64> {
    let mut qpc = 0i64;
    unsafe { windows::Win32::System::Performance::QueryPerformanceCounter(&mut qpc) }
        .context("QueryPerformanceCounter failed")?;
    Ok(qpc)
}

/// Base hardware encoder using FFmpeg CLI
pub struct HardwareEncoderBase {
    config: EncoderConfig,
    encoder_name: String,
    packet_rx: Receiver<EncodedPacket>,
    packet_tx: Sender<EncodedPacket>,
    frame_count: u64,
    running: bool,
    ffmpeg_process: Option<std::process::Child>,
    ffmpeg_stdin: Option<ChildStdin>,
    width: u32,
    height: u32,
    /// JoinHandle for the stdout reader thread
    stdout_thread: Option<thread::JoinHandle<()>>,
    /// JoinHandle for the stderr reader thread
    stderr_thread: Option<thread::JoinHandle<()>>,
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

        // Determine resolution
        let (width, height) = if config.use_native_resolution {
            // Will be set on first frame
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
    fn init_ffmpeg(&mut self, width: u32, height: u32) -> Result<()> {
        let ffmpeg_cmd = resolve_ffmpeg_command();
        let encoder_name = self.encoder_name.as_str();

        // Determine output resolution: use config if set, otherwise native
        let (out_w, out_h) = if !self.config.use_native_resolution
            && self.config.resolution.0 > 0
            && self.config.resolution.1 > 0
        {
            self.config.resolution
        } else {
            (width, height)
        };

        // Build FFmpeg command based on encoder type
        let mut cmd = Command::new(&ffmpeg_cmd);
        cmd.arg("-y");

        let mut video_filters: Vec<String> = Vec::new();

        if self.config.use_cpu_readback {
            // Push-model input from app capture thread.
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
                // Ensure constant frame rate timing
                .arg("-vsync")
                .arg("cfr");
        } else {
            // Pull-model input: FFmpeg captures desktop directly via ddagrab.
            cmd.arg("-f")
                .arg("lavfi")
                .arg("-i")
                .arg(format!(
                    "ddagrab=output_idx={}:framerate={}",
                    self.config.output_index, self.config.framerate
                ))
                // Ensure constant frame rate timing
                .arg("-vsync")
                .arg("cfr");

            // ddagrab frames are D3D11 hardware surfaces; AMF path needs download
            // into system memory to avoid unsupported auto-scale graph initialization.
            if matches!(encoder_name, "h264_amf" | "hevc_amf" | "av1_amf") {
                video_filters.push("hwdownload,format=bgra".to_string());
            }
        }

        // Scale inside FFmpeg when output differs from input
        if out_w != width || out_h != height {
            video_filters.push(format!("scale={}:{}", out_w, out_h));
        }

        if !video_filters.is_empty() {
            cmd.arg("-vf").arg(video_filters.join(","));
        }

        cmd
            // Output: H.264 NAL units (no container, just raw stream)
            .arg("-c:v")
            .arg(encoder_name);

        // Add encoder-specific options immediately after -c:v (required for AMF)
        self.add_encoder_options(&mut cmd, encoder_name);

        // Add preset if supported by the encoder
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
            .arg(self.config.framerate.to_string()) // Explicit output frame rate
            .arg("-vsync")
            .arg("cfr") // Constant frame rate to maintain timing
            .arg("-f")
            .arg("h264")
            .arg("pipe:1");

        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        // Capture stderr for debugging - helps diagnose FFmpeg failures
        cmd.stderr(Stdio::piped());

        // Log the full command for debugging (only format if info level is enabled)
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

        // Create channel for frame metadata (timestamps) to output reader
        let (frame_meta_tx, frame_meta_rx) = bounded::<FrameMetadata>(256);
        self.frame_meta_tx = Some(frame_meta_tx);

        // Start the output reader thread
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

        let stdout_handle =
            self.spawn_output_reader(stdout, packet_tx, frame_meta_rx, start_qpc);
        self.stdout_thread = Some(stdout_handle);

        // Spawn stderr reader to capture FFmpeg diagnostic output
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
            "h264_amf" | "hevc_amf" | "av1_amf" => "", // AMF uses -quality instead, not -preset
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
                // NVENC-specific rate control and latency behavior.
                cmd.arg("-rc");
                cmd.arg(self.nvenc_rate_control_mode());
                if matches!(self.config.rate_control, RateControl::Cq) {
                    cmd.arg("-cq");
                    cmd.arg(self.cq_value().to_string());
                }

                // Optimize for performance/latency
                cmd.arg("-delay");
                cmd.arg("0"); // No delay for lowest latency

                // Use faster presets for higher framerates
                cmd.arg("-tune");
                cmd.arg("ull"); // Ultra-low latency mode

                // Reduce B-frame usage for lower latency
                cmd.arg("-b_ref_mode");
                cmd.arg("disabled");
            }
            "h264_amf" | "hevc_amf" | "av1_amf" => {
                // AMF-specific quality mode. Keep required compatibility flags.
                cmd.arg("-quality");
                cmd.arg(self.amf_quality_mode());
                // Disable B-frames for low latency (critical for AMF)
                cmd.arg("-bf");
                cmd.arg("0");
                // Force SPS/PPS insertion with every keyframe
                // This is critical for hardware encoders to produce valid H.264
                cmd.arg("-sei");
                cmd.arg("+aud");
                // Ensure consistent frame rate
                cmd.arg("-vsync");
                cmd.arg("cfr");

                // Optimize for performance
                cmd.arg("-usage");
                cmd.arg("lowlatency");
            }
            "h264_qsv" | "hevc_qsv" => {
                // QSV specific: no look ahead for lower latency
                cmd.arg("-look_ahead");
                cmd.arg("0");

                // Optimize for performance
                cmd.arg("-preset");
                cmd.arg("faster"); // Use faster preset for higher framerates
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

                // NVENC CQ works best with unconstrained average bitrate.
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
        let qpc_freq = qpc_frequency(); // Use the cached QPC frequency

        thread::spawn(move || {
            use std::io::Read;

            let mut reader = std::io::BufReader::new(stdout);
            let mut buffer = [0u8; 65536]; // 64KB buffer
            let mut frame_buffer = BytesMut::with_capacity(1024 * 1024); // Pre-allocate 1MB
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

                        // Extract NAL units
                        loop {
                            let Some((start_pos, start_len)) =
                                find_annexb_start_code(&frame_buffer, 0)
                            else {
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

                            let Some((next_start, _)) =
                                find_annexb_start_code(&frame_buffer, start_len)
                            else {
                                break;
                            };

                            let nal_data = frame_buffer.split_to(next_start).freeze();

                            if !nal_data.is_empty() {
                                let nal_type = h264_nal_type(&nal_data);
                                let is_keyframe = matches!(nal_type, Some(5 | 7 | 8));
                                let is_frame_nal = matches!(nal_type, Some(1 | 5));

                                let final_pts = if is_frame_nal {
                                    // Receive the next frame metadata to get original capture timestamp
                                    match frame_meta_rx.try_recv() {
                                    Ok(meta) => {
                                        // Calculate PTS based on original capture timestamp
                                            // This preserves A/V sync by using the same timeline as audio
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
                                            // No metadata available, fallback to elapsed time
                                            // This can happen if FFmpeg outputs more NALs than frames sent
                                            let elapsed_qpc = (output_reader_start.elapsed().as_secs_f64()
                                                * qpc_freq as f64)
                                                as i64;
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
                                    // Parameter-set NALs (SPS/PPS) before first frame.
                                    start_qpc
                                } else {
                                    // For non-frame NALs (AUD, SEI, etc.), use the current frame's timestamp
                                    // or the last known timestamp to keep them aligned
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
    fn spawn_stderr_reader(&self, stderr: std::process::ChildStderr) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                debug!("[FFmpeg] {}", line);
            }
            debug!("FFmpeg stderr reader thread exiting");
        })
    }
    /// Encode a single frame
    fn encode_frame_internal(
        &mut self,
        frame: &super::super::capture::CapturedFrame,
    ) -> Result<()> {
        // Initialize FFmpeg on first frame if not done.
        // Always use the native frame resolution for the FFmpeg rawvideo input
        // because we send the raw BGRA bytes as-is (no CPU-side resize).
        // If a smaller output is desired, FFmpeg scales internally via -vf.
        if self.ffmpeg_process.is_none() {
            let (native_w, native_h) = frame.resolution;
            self.init_ffmpeg(native_w, native_h)?;
        }

        // Send frame metadata (timestamp) to output reader for A/V sync
        if let Some(ref meta_tx) = self.frame_meta_tx {
            let meta = FrameMetadata {
                capture_timestamp: frame.timestamp,
            };
            // Use try_send to avoid blocking capture thread if reader is behind
            if meta_tx.try_send(meta).is_err() && self.frame_count % 60 == 0 {
                debug!("Frame metadata channel full, dropping timestamp for frame {}", self.frame_count);
            }
        }

        if !self.config.use_cpu_readback {
            // Pull mode captures desktop in FFmpeg directly; no per-frame stdin writes.
            self.frame_count += 1;
            return Ok(());
        }

        // Write frame to FFmpeg stdin (always native resolution bytes)
        if let Some(ref mut stdin) = self.ffmpeg_stdin {
            let data = &frame.bgra;

            stdin
                .write_all(data)
                .context("Failed to write frame to FFmpeg stdin")?;

            self.frame_count += 1;
            if self.frame_count % 60 == 0 {
                trace!("Encoded frame {} to FFmpeg", self.frame_count);
            }
        }

        Ok(())
    }

    /// Flush remaining frames and close FFmpeg
    fn flush_internal(&mut self) -> Result<Vec<EncodedPacket>> {
        // Drop stdin to signal EOF to FFmpeg
        if let Some(stdin) = self.ffmpeg_stdin.take() {
            drop(stdin);
        }

        // Wait for FFmpeg to finish with a timeout to prevent hanging
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
                            warn!("FFmpeg process did not exit within {:?}, killing", timeout);
                            let _ = child.kill();
                            let _ = child.wait(); // reap after kill
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

        // Join reader threads with a timeout
        let join_timeout = std::time::Duration::from_secs(5);
        if let Some(handle) = self.stdout_thread.take() {
            let start = Instant::now();
            while !handle.is_finished() {
                if start.elapsed() > join_timeout {
                    warn!(
                        "stdout reader thread did not finish within {:?}",
                        join_timeout
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
                        "stderr reader thread did not finish within {:?}",
                        join_timeout
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

        // Drain remaining packets
        let mut packets = Vec::new();
        while let Ok(packet) = self.packet_rx.try_recv() {
            packets.push(packet);
        }

        info!("Flushed {} packets from hardware encoder", packets.len());
        Ok(packets)
    }
}

/// Drop implementation to ensure FFmpeg process is cleaned up
impl Drop for HardwareEncoderBase {
    fn drop(&mut self) {
        // Close stdin to signal EOF
        drop(self.ffmpeg_stdin.take());

        // Kill FFmpeg process if still running
        if let Some(mut child) = self.ffmpeg_process.take() {
            match child.try_wait() {
                Ok(Some(_)) => {} // already exited
                _ => {
                    warn!("FFmpeg process still running during drop, killing");
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }

        // Best-effort join reader threads (don't block long)
        if let Some(handle) = self.stdout_thread.take() {
            if handle.is_finished() {
                let _ = handle.join();
            }
        }
        if let Some(handle) = self.stderr_thread.take() {
            if handle.is_finished() {
                let _ = handle.join();
            }
        }
    }
}

/// NVENC encoder wrapper (NVIDIA)
pub struct NvencEncoder {
    base: HardwareEncoderBase,
}

impl NvencEncoder {
    pub fn new(config: &EncoderConfig) -> Result<Self> {
        debug!("Creating NVENC encoder");
        let base = HardwareEncoderBase::new(config, "h264_nvenc")?;
        Ok(Self { base })
    }
}

impl Encoder for NvencEncoder {
    fn init(&mut self, config: &EncoderConfig) -> Result<()> {
        self.base.config = config.clone();
        self.base.running = true;
        if !self.base.config.use_cpu_readback && self.base.ffmpeg_process.is_none() {
            self.base.init_ffmpeg(0, 0)?;
        }
        debug!("NVENC encoder initialized");
        Ok(())
    }

    fn encode_frame(&mut self, frame: &super::super::capture::CapturedFrame) -> Result<()> {
        self.base.encode_frame_internal(frame)
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>> {
        self.base.flush_internal()
    }

    fn packet_rx(&self) -> Receiver<EncodedPacket> {
        self.base.packet_rx.clone()
    }

    fn is_running(&self) -> bool {
        self.base.running
    }
}

/// AMF encoder wrapper (AMD)
pub struct AmfEncoder {
    base: HardwareEncoderBase,
}

impl AmfEncoder {
    pub fn new(config: &EncoderConfig) -> Result<Self> {
        debug!("Creating AMF encoder");
        let base = HardwareEncoderBase::new(config, "h264_amf")?;
        Ok(Self { base })
    }
}

impl Encoder for AmfEncoder {
    fn init(&mut self, config: &EncoderConfig) -> Result<()> {
        self.base.config = config.clone();
        self.base.running = true;
        if !self.base.config.use_cpu_readback && self.base.ffmpeg_process.is_none() {
            self.base.init_ffmpeg(0, 0)?;
        }
        debug!("AMF encoder initialized");
        Ok(())
    }

    fn encode_frame(&mut self, frame: &super::super::capture::CapturedFrame) -> Result<()> {
        self.base.encode_frame_internal(frame)
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>> {
        self.base.flush_internal()
    }

    fn packet_rx(&self) -> Receiver<EncodedPacket> {
        self.base.packet_rx.clone()
    }

    fn is_running(&self) -> bool {
        self.base.running
    }
}

/// QSV encoder wrapper (Intel)
pub struct QsvEncoder {
    base: HardwareEncoderBase,
}

impl QsvEncoder {
    pub fn new(config: &EncoderConfig) -> Result<Self> {
        debug!("Creating QSV encoder");
        let base = HardwareEncoderBase::new(config, "h264_qsv")?;
        Ok(Self { base })
    }
}

impl Encoder for QsvEncoder {
    fn init(&mut self, config: &EncoderConfig) -> Result<()> {
        self.base.config = config.clone();
        self.base.running = true;
        if !self.base.config.use_cpu_readback && self.base.ffmpeg_process.is_none() {
            self.base.init_ffmpeg(0, 0)?;
        }
        debug!("QSV encoder initialized");
        Ok(())
    }

    fn encode_frame(&mut self, frame: &super::super::capture::CapturedFrame) -> Result<()> {
        self.base.encode_frame_internal(frame)
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>> {
        self.base.flush_internal()
    }

    fn packet_rx(&self) -> Receiver<EncodedPacket> {
        self.base.packet_rx.clone()
    }

    fn is_running(&self) -> bool {
        self.base.running
    }
}

/// Check if a hardware encoder is available by attempting to probe it
#[cfg(feature = "ffmpeg")]
pub fn check_encoder_available(encoder_name: &str) -> bool {
    let ffmpeg_cmd = resolve_ffmpeg_command();

    // First check if encoder is listed
    let output = Command::new(&ffmpeg_cmd)
        .arg("-hide_banner")
        .arg("-encoders")
        .arg("-v")
        .arg("error")
        .output();

    let listed = match output {
        Ok(out) => {
            let output_str = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            output_str.contains(encoder_name)
        }
        Err(e) => {
            warn!(
                "Failed to check encoder listing for {}: {}",
                encoder_name, e
            );
            return false;
        }
    };

    if !listed {
        info!("Encoder {} not found in FFmpeg", encoder_name);
        return false;
    }

    // Probe the encoder by attempting a tiny encode to verify it actually works
    // (catches cases where encoder is listed but GPU driver is missing/broken)
    let mut probe_cmd = Command::new(&ffmpeg_cmd);
    probe_cmd
        .arg("-hide_banner")
        .arg("-v")
        .arg("error")
        .arg("-f")
        .arg("lavfi")
        .arg("-i")
        .arg("nullsrc=s=320x240:d=0.04")
        .arg("-c:v")
        .arg(encoder_name)
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg("-frames:v")
        .arg("1");

    // Add encoder-specific options for the probe
    match encoder_name {
        "h264_amf" | "hevc_amf" | "av1_amf" => {
            probe_cmd.arg("-quality").arg("speed");
            probe_cmd.arg("-bf").arg("0");
        }
        "h264_nvenc" | "hevc_nvenc" | "av1_nvenc" => {
            probe_cmd.arg("-preset").arg("p4");
        }
        "h264_qsv" | "hevc_qsv" => {
            probe_cmd.arg("-preset").arg("veryfast");
        }
        _ => {}
    }

    // Log the probe command for debugging
    debug!("Probing encoder {} with FFmpeg", encoder_name);

    let probe = probe_cmd.arg("-f").arg("null").arg("-").output();

    match probe {
        Ok(out) => {
            if out.status.success() {
                info!("Encoder {} probe succeeded", encoder_name);
                true
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr);
                info!(
                    "Encoder {} is listed but probe failed - may indicate missing/broken driver",
                    encoder_name
                );
                warn!("Encoder {} probe failed: {}", encoder_name, stderr.trim());
                false
            }
        }
        Err(e) => {
            warn!("Failed to probe encoder {}: {}", encoder_name, e);
            false
        }
    }
}

#[cfg(not(feature = "ffmpeg"))]
pub fn check_encoder_available(_encoder_name: &str) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> EncoderConfig {
        EncoderConfig::new(
            crate::config::Codec::H264,
            20,
            30,
            (1920, 1080),
            crate::config::EncoderType::Nvenc,
            1,
        )
    }

    #[test]
    fn test_nvenc_encoder_creation() {
        let config = create_test_config();
        let encoder = NvencEncoder::new(&config);
        assert!(encoder.is_ok());
    }

    #[test]
    fn test_amf_encoder_creation() {
        let config = EncoderConfig::new(
            crate::config::Codec::H264,
            20,
            30,
            (1920, 1080),
            crate::config::EncoderType::Amf,
            1,
        );
        let encoder = AmfEncoder::new(&config);
        assert!(encoder.is_ok());
    }

    #[test]
    fn test_qsv_encoder_creation() {
        let config = EncoderConfig::new(
            crate::config::Codec::H264,
            20,
            30,
            (1920, 1080),
            crate::config::EncoderType::Qsv,
            1,
        );
        let encoder = QsvEncoder::new(&config);
        assert!(encoder.is_ok());
    }

    #[test]
    fn test_bgra_to_rgb24() {
        // Test BGRA to RGB24 conversion
        let bgra: Vec<u8> = vec![
            0x00, 0x00, 0xFF, 0xFF, // BGRA: B=0, G=0, R=255, A=255 (Red)
            0x00, 0xFF, 0x00, 0xFF, // BGRA: B=0, G=255, R=0, A=255 (Green)
            0xFF, 0x00, 0x00, 0xFF, // BGRA: B=255, G=0, R=0, A=255 (Blue)
        ];
        let rgb = bgra_to_rgb24(&bgra, 3, 1);

        // Expected: R, G, B
        assert_eq!(rgb.len(), 9);
        assert_eq!(rgb[0], 0xFF); // R from first pixel
        assert_eq!(rgb[1], 0x00); // G from first pixel
        assert_eq!(rgb[2], 0x00); // B from first pixel
    }

    #[test]
    fn test_quality_preset_maps_to_nvenc_presets() {
        let mut config = create_test_config();
        config.quality_preset = QualityPreset::Performance;
        let base = HardwareEncoderBase::new(&config, "h264_nvenc").expect("base");
        assert_eq!(base.preset_for_encoder("h264_nvenc"), "p3");

        config.quality_preset = QualityPreset::Balanced;
        let base = HardwareEncoderBase::new(&config, "h264_nvenc").expect("base");
        assert_eq!(base.preset_for_encoder("h264_nvenc"), "p5");

        config.quality_preset = QualityPreset::Quality;
        let base = HardwareEncoderBase::new(&config, "h264_nvenc").expect("base");
        assert_eq!(base.preset_for_encoder("h264_nvenc"), "p7");
    }

    #[test]
    fn test_cq_default_value_uses_quality_preset() {
        let mut config = create_test_config();
        config.rate_control = RateControl::Cq;
        config.quality_preset = QualityPreset::Quality;
        config.quality_value = None;

        let base = HardwareEncoderBase::new(&config, "h264_nvenc").expect("base");
        assert_eq!(base.cq_value(), 19);
    }
}
