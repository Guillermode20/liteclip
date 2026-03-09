//! Auto-generated module
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use anyhow::{Context, Result};
use crossbeam::channel::{bounded, Receiver, Sender};
#[cfg(feature = "ffmpeg")]
use ffmpeg::format::Pixel;
#[cfg(feature = "ffmpeg")]
use ffmpeg_next as ffmpeg;
use tracing::{debug, info, warn};
#[cfg(windows)]
use windows::Win32::System::Threading::{
    GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL,
};

use super::types::{
    EncodedPacket, EncoderConfig, EncoderHandle, EncoderHealthEvent, HardwareEncoder,
};
/// Encoder trait
///
/// All encoders must be Send + 'static as they run on dedicated threads.
/// The encoder receives CapturedFrame from the capture thread and outputs
/// EncodedPacket via the channel returned by packet_rx().
pub trait Encoder: Send + 'static {
    /// Initialize encoder with configuration
    fn init(&mut self, config: &EncoderConfig) -> Result<()>;
    /// Encode a frame
    fn encode_frame(&mut self, frame: &crate::capture::CapturedFrame) -> Result<()>;
    /// Flush encoder and get remaining packets
    fn flush(&mut self) -> Result<Vec<EncodedPacket>>;
    /// Get receiver for encoded packets
    fn packet_rx(&self) -> Receiver<EncodedPacket>;
    /// Check if encoder is still running
    fn is_running(&self) -> bool;
}

fn spawn_buffer_writer(
    buffer: crate::buffer::ring::SharedReplayBuffer,
) -> (Sender<EncodedPacket>, std::thread::JoinHandle<(u64, usize)>) {
    let (packet_tx, packet_rx) = bounded::<EncodedPacket>(2048);
    let thread = std::thread::Builder::new()
        .name("encoder-buffer-writer".to_string())
        .spawn(move || {
            let mut total_packets = 0u64;
            let mut flush_batches = 0usize;
            let mut packet_batch = Vec::with_capacity(256);

            while let Ok(packet) = packet_rx.recv() {
                packet_batch.push(packet);
                total_packets = total_packets.saturating_add(1);

                while packet_batch.len() < 256 {
                    match packet_rx.try_recv() {
                        Ok(packet) => {
                            packet_batch.push(packet);
                            total_packets = total_packets.saturating_add(1);
                        }
                        Err(_) => break,
                    }
                }

                buffer.push_batch(packet_batch.drain(..));
                flush_batches = flush_batches.saturating_add(1);
            }

            if !packet_batch.is_empty() {
                buffer.push_batch(packet_batch.drain(..));
                flush_batches = flush_batches.saturating_add(1);
            }

            (total_packets, flush_batches)
        })
        .expect("failed to spawn encoder buffer writer thread");

    (packet_tx, thread)
}

fn forward_ready_packets(
    packet_rx: &Receiver<EncodedPacket>,
    buffer_packet_tx: &Sender<EncodedPacket>,
) -> u64 {
    let mut forwarded = 0u64;
    while let Ok(packet) = packet_rx.try_recv() {
        if buffer_packet_tx.send(packet).is_err() {
            break;
        }
        forwarded = forwarded.saturating_add(1);
    }
    forwarded
}

fn resolve_encoder_config(config: &EncoderConfig) -> EncoderConfig {
    let mut resolved = config.clone();
    if resolved.encoder_type == crate::config::EncoderType::Auto {
        let detected_encoder = detect_hardware_encoder();
        apply_auto_encoder_selection(&mut resolved, detected_encoder);
        info!("Auto-detected encoder: {:?}", resolved.encoder_type);
    }
    resolved
}

pub fn resolve_effective_encoder_config(config: &EncoderConfig) -> EncoderConfig {
    resolve_encoder_config(config)
}

fn set_encoder_thread_priority() {
    #[cfg(windows)]
    {
        unsafe {
            if let Err(e) = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL) {
                warn!("Failed to raise encoder thread priority: {}", e);
            }
        }
    }
}

#[cfg(feature = "ffmpeg")]
fn probe_encoder_available(encoder_name: &str) -> bool {
    let Some(codec) = ffmpeg::encoder::find_by_name(encoder_name) else {
        debug!(
            "Encoder {} not present in linked FFmpeg build",
            encoder_name
        );
        return false;
    };

    let encoder_ctx = ffmpeg::codec::context::Context::new_with_codec(codec);
    let mut encoder = match encoder_ctx.encoder().video() {
        Ok(encoder) => encoder,
        Err(error) => {
            debug!(
                "Encoder {} exists but could not create a video context: {}",
                encoder_name, error
            );
            return false;
        }
    };

    encoder.set_width(320);
    encoder.set_height(240);
    encoder.set_format(Pixel::YUV420P);
    encoder.set_frame_rate(Some((30, 1)));
    encoder.set_time_base((1, 30));
    encoder.set_bit_rate(2_000_000);
    encoder.set_max_bit_rate(2_000_000);
    encoder.set_gop(30);

    let mut options = ffmpeg::Dictionary::new();
    options.set("bf", "0");

    match encoder_name {
        "h264_amf" | "hevc_amf" | "av1_amf" => {
            options.set("usage", "lowlatency");
            options.set("quality", "speed");
            options.set("preanalysis", "0");
        }
        "h264_nvenc" | "hevc_nvenc" | "av1_nvenc" => {
            options.set("preset", "p4");
            options.set("tune", "ll");
            options.set("zerolatency", "1");
        }
        "h264_qsv" | "hevc_qsv" => {
            options.set("preset", "veryfast");
            options.set("look_ahead", "0");
        }
        _ => {}
    }

    match encoder.open_with(options) {
        Ok(_) => {
            info!("Native probe succeeded for encoder {}", encoder_name);
            true
        }
        Err(error) => {
            debug!(
                "Native probe failed for encoder {}: {}",
                encoder_name, error
            );
            false
        }
    }
}

/// Detect available hardware encoder (HEVC-only)
///
/// Priority order: NVENC → AMF → QSV
#[cfg(feature = "ffmpeg")]
pub fn detect_hardware_encoder() -> HardwareEncoder {
    debug!("Detecting hardware encoders for HEVC...");

    // HEVC encoders only
    let nvenc_name = "hevc_nvenc";
    let amf_name = "hevc_amf";
    let qsv_name = "hevc_qsv";

    if probe_encoder_available(nvenc_name) {
        info!("Using NVIDIA NVENC encoder");
        return HardwareEncoder::Nvenc;
    }
    if probe_encoder_available(amf_name) {
        info!("Using AMD AMF encoder");
        return HardwareEncoder::Amf;
    }
    if probe_encoder_available(qsv_name) {
        info!("Using Intel QSV encoder");
        return HardwareEncoder::Qsv;
    }
    warn!("No hardware encoder found");
    HardwareEncoder::None
}
/// Detect available hardware encoder (non-FFmpeg fallback)
#[cfg(not(feature = "ffmpeg"))]
pub fn detect_hardware_encoder() -> HardwareEncoder {
    warn!("FFmpeg not compiled in, no hardware encoder available");
    HardwareEncoder::None
}

pub(super) fn apply_auto_encoder_selection(
    config: &mut EncoderConfig,
    detected_encoder: HardwareEncoder,
) {
    config.encoder_type = detected_encoder.into();

    // AMF-specific optimizations for realtime stability
    if matches!(detected_encoder, HardwareEncoder::Amf) && config.keyframe_interval_secs < 2 {
        warn!(
            "Raising keyframe interval from {}s to 2s for AMD AMF realtime stability",
            config.keyframe_interval_secs
        );
        config.keyframe_interval_secs = 2;
    }

    if matches!(detected_encoder, HardwareEncoder::Amf)
        && config.framerate >= 60
        && matches!(
            config.quality_preset,
            crate::config::QualityPreset::Balanced
        )
    {
        warn!(
            "Switching AMD AMF preset from Balanced to Performance for 1080p60 realtime stability"
        );
        config.quality_preset = crate::config::QualityPreset::Performance;
    }
}

/// Check if a specific FFmpeg encoder is available
#[allow(dead_code)]
fn is_encoder_available(codec_name: &str) -> bool {
    debug!(
        "Checking encoder availability: {} (FFmpeg not compiled in)",
        codec_name
    );
    false
}
/// Create the best available encoder based on configuration
///
/// If encoder_type is Auto, performs hardware detection.
/// Falls back to software encoder if hardware initialization fails.
pub fn create_encoder(config: &EncoderConfig) -> Result<Box<dyn Encoder>> {
    let config = resolve_encoder_config(config);

    info!("Creating native FFmpeg encoder: {:?}", config.encoder_type);
    Ok(Box::new(crate::encode::ffmpeg::FfmpegEncoder::new(
        &config,
    )?))
}
/// Spawn an encoder on a dedicated thread
///
/// This creates the encoder, initializes it, and runs it on a new thread.
/// Frames are sent via the returned Sender, and encoded packets are pushed
/// directly to the SharedReplayBuffer.
pub fn spawn_encoder(
    config: EncoderConfig,
    buffer: crate::buffer::ring::SharedReplayBuffer,
) -> Result<(EncoderHandle, Sender<crate::capture::CapturedFrame>)> {
    const MAX_CONSECUTIVE_ENCODE_ERRORS: u32 = 8;
    let effective_config = resolve_encoder_config(&config);
    let thread_config = effective_config.clone();
    let (frame_tx, frame_rx): (Sender<crate::capture::CapturedFrame>, _) = bounded(128);
    let frame_tx_clone = frame_tx.clone();
    let (health_tx, health_rx) = bounded(8);
    let thread = std::thread::Builder::new()
        .name("encoder".to_string())
        .stack_size(4 * 1024 * 1024)
        .spawn(move || {
            set_encoder_thread_priority();
            debug!("Encoder thread started");
            let mut encoder = match create_encoder(&thread_config) {
                Ok(encoder) => encoder,
                Err(e) => {
                    let _ = health_tx.try_send(EncoderHealthEvent::Fatal(format!(
                        "Failed to create encoder: {}",
                        e
                    )));
                    return Err(e);
                }
            };
            if let Err(init_error) = encoder.init(&thread_config) {
                let _ = health_tx.try_send(EncoderHealthEvent::Fatal(format!(
                    "Failed to initialize encoder: {}",
                    init_error
                )));
                return Err(init_error);
            }
            debug!("Encoder initialized");
            let packet_rx = encoder.packet_rx();
            let (buffer_packet_tx, buffer_writer_thread) = spawn_buffer_writer(buffer.clone());
            let mut frames_encoded = 0u64;
            let mut packets_received = 0u64;
            let mut consecutive_encode_errors = 0u32;
            const MAX_FRAME_BURST: usize = 8;
            loop {
                packets_received += forward_ready_packets(&packet_rx, &buffer_packet_tx);
                match frame_rx.recv_timeout(std::time::Duration::from_millis(8)) {
                    Ok(frame) => {
                        let mut encode_one = |frame: crate::capture::CapturedFrame| -> Result<()> {
                            frames_encoded += 1;
                            if let Err(e) = encoder.encode_frame(&frame) {
                                warn!("Failed to encode frame: {}", e);
                                consecutive_encode_errors =
                                    consecutive_encode_errors.saturating_add(1);
                                if consecutive_encode_errors >= MAX_CONSECUTIVE_ENCODE_ERRORS {
                                    let reason = format!(
                                        "{} consecutive frame encode failures",
                                        consecutive_encode_errors
                                    );
                                    let _ = health_tx
                                        .try_send(EncoderHealthEvent::Fatal(reason.clone()));
                                    return Err(anyhow::anyhow!(reason));
                                }
                            } else {
                                consecutive_encode_errors = 0;
                            }
                            packets_received +=
                                forward_ready_packets(&packet_rx, &buffer_packet_tx);
                            Ok(())
                        };

                        encode_one(frame)?;

                        for _ in 1..MAX_FRAME_BURST {
                            let Ok(frame) = frame_rx.try_recv() else {
                                break;
                            };
                            encode_one(frame)?;
                        }
                    }
                    Err(crossbeam::channel::RecvTimeoutError::Timeout) => {
                        packets_received += forward_ready_packets(&packet_rx, &buffer_packet_tx);
                    }
                    Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                        debug!("Frame channel closed, shutting down encoder");
                        break;
                    }
                }
            }
            info!(
                "Encoder loop ended: {} frames encoded, {} packets received",
                frames_encoded, packets_received
            );
            debug!("Encoder thread shutting down");
            match encoder.flush() {
                Ok(packets) => {
                    info!("Flushed {} packets from encoder", packets.len());
                    for packet in packets {
                        buffer_packet_tx.send(packet).unwrap();
                    }
                }
                Err(e) => {
                    warn!("Failed to flush encoder: {}", e);
                }
            }
            let mut final_packets = Vec::new();
            while let Ok(packet) = packet_rx.try_recv() {
                buffer_packet_tx.send(packet).unwrap();
                final_packets.push(());
            }
            info!(
                "Drained {} final packets from encoder channel",
                final_packets.len()
            );
            drop(buffer_packet_tx);
            if let Ok((buffered_packets, flush_batches)) = buffer_writer_thread.join() {
                info!(
                    "Encoder buffer writer flushed {} packets across {} batches",
                    buffered_packets, flush_batches
                );
            }
            debug!("Encoder thread stopped");
            Ok(())
        })?;
    let (_, dummy_packet_rx) = bounded(1);
    let handle = EncoderHandle {
        thread,
        frame_tx: frame_tx_clone.clone(),
        packet_rx: dummy_packet_rx,
        health_rx,
        effective_config,
    };
    Ok((handle, frame_tx_clone))
}
/// Spawn an encoder that receives frames from an existing receiver
///
/// This is used when the capture provides its own frame channel.
/// The encoder thread reads frames from frame_rx and pushes encoded packets
/// directly to the SharedReplayBuffer.
pub fn spawn_encoder_with_receiver(
    config: EncoderConfig,
    buffer: crate::buffer::ring::SharedReplayBuffer,
    frame_rx: Receiver<crate::capture::CapturedFrame>,
) -> Result<EncoderHandle> {
    const MAX_CONSECUTIVE_ENCODE_ERRORS: u32 = 8;
    let effective_config = resolve_encoder_config(&config);
    let thread_config = effective_config.clone();
    let (health_tx, health_rx) = bounded(8);
    let thread = std::thread::Builder::new()
        .name("encoder".to_string())
        .stack_size(4 * 1024 * 1024)
        .spawn(move || {
            set_encoder_thread_priority();
            debug!("Encoder thread started");
            let mut encoder = match create_encoder(&thread_config) {
                Ok(encoder) => encoder,
                Err(e) => {
                    let _ = health_tx.try_send(EncoderHealthEvent::Fatal(format!(
                        "Failed to create encoder: {}",
                        e
                    )));
                    return Err(e);
                }
            };
            if let Err(init_error) = encoder.init(&thread_config) {
                let _ = health_tx.try_send(EncoderHealthEvent::Fatal(format!(
                    "Failed to initialize encoder: {}",
                    init_error
                )));
                return Err(init_error);
            }
            debug!("Encoder initialized");
            let packet_rx = encoder.packet_rx();
            let (buffer_packet_tx, buffer_writer_thread) = spawn_buffer_writer(buffer.clone());
            let mut consecutive_encode_errors = 0u32;
            const MAX_FRAME_BURST: usize = 8;
            loop {
                let _ = forward_ready_packets(&packet_rx, &buffer_packet_tx);
                match frame_rx.recv_timeout(std::time::Duration::from_millis(8)) {
                    Ok(frame) => {
                        let mut encode_one = |frame: crate::capture::CapturedFrame| -> Result<()> {
                            if let Err(e) = encoder.encode_frame(&frame) {
                                warn!("Failed to encode frame: {}", e);
                                consecutive_encode_errors =
                                    consecutive_encode_errors.saturating_add(1);
                                if consecutive_encode_errors >= MAX_CONSECUTIVE_ENCODE_ERRORS {
                                    let reason = format!(
                                        "{} consecutive frame encode failures",
                                        consecutive_encode_errors
                                    );
                                    let _ = health_tx
                                        .try_send(EncoderHealthEvent::Fatal(reason.clone()));
                                    return Err(anyhow::anyhow!(reason));
                                }
                            } else {
                                consecutive_encode_errors = 0;
                            }
                            let _ = forward_ready_packets(&packet_rx, &buffer_packet_tx);
                            Ok(())
                        };

                        encode_one(frame)?;

                        for _ in 1..MAX_FRAME_BURST {
                            let Ok(frame) = frame_rx.try_recv() else {
                                break;
                            };
                            encode_one(frame)?;
                        }
                    }
                    Err(crossbeam::channel::RecvTimeoutError::Timeout) => {
                        let _ = forward_ready_packets(&packet_rx, &buffer_packet_tx);
                    }
                    Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                        debug!("Frame channel closed, shutting down encoder");
                        break;
                    }
                }
            }
            debug!("Encoder thread shutting down");
            match encoder.flush() {
                Ok(packets) => {
                    for packet in packets {
                        if buffer_packet_tx.send(packet).is_err() {
                            break;
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to flush encoder: {}", e);
                }
            }
            let mut final_packets = Vec::new();
            while let Ok(packet) = packet_rx.try_recv() {
                if buffer_packet_tx.send(packet).is_ok() {
                    final_packets.push(());
                }
            }
            debug!(
                "Drained {} final packets from encoder channel",
                final_packets.len()
            );
            drop(buffer_packet_tx);
            if let Ok((buffered_packets, flush_batches)) = buffer_writer_thread.join() {
                debug!(
                    "Encoder buffer writer flushed {} packets across {} batches",
                    buffered_packets, flush_batches
                );
            }
            debug!("Encoder thread stopped");
            Ok(())
        })?;
    let (_, dummy_packet_rx) = bounded(1);
    let (dummy_frame_tx, _) = bounded(1);
    let handle = EncoderHandle {
        thread,
        frame_tx: dummy_frame_tx,
        packet_rx: dummy_packet_rx,
        health_rx,
        effective_config,
    };
    Ok(handle)
}
/// Initialize FFmpeg (call once at startup)
#[cfg(feature = "ffmpeg")]
pub fn init_ffmpeg() -> Result<()> {
    ffmpeg_next::init().context("Failed to initialize FFmpeg")?;
    info!("FFmpeg initialized successfully");
    Ok(())
}

/// Initialize FFmpeg (call once at startup)
#[cfg(not(feature = "ffmpeg"))]
pub fn init_ffmpeg() -> Result<()> {
    info!("FFmpeg initialization skipped (not compiled in)");
    Ok(())
}
impl From<HardwareEncoder> for crate::config::EncoderType {
    fn from(hw: HardwareEncoder) -> Self {
        match hw {
            HardwareEncoder::Nvenc => crate::config::EncoderType::Nvenc,
            HardwareEncoder::Amf => crate::config::EncoderType::Amf,
            HardwareEncoder::Qsv => crate::config::EncoderType::Qsv,
            HardwareEncoder::None => crate::config::EncoderType::Amf, // Default to AMF as fallback
        }
    }
}
