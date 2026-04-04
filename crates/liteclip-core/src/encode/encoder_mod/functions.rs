//! Encoder spawn, resolution, and trait definitions.

use crate::encode::{EncodeError, EncodeResult};
use crossbeam::channel::{bounded, Receiver};
#[cfg(feature = "ffmpeg")]
use ffmpeg_next as ffmpeg;
#[cfg(feature = "ffmpeg")]
use ffmpeg_next::format::Pixel;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};
#[cfg(windows)]
use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority};

use super::types::{
    EncodedPacket, EncoderConfig, EncoderHandle, EncoderHealthEvent, HardwareEncoder,
    ResolvedEncoderConfig, ResolvedEncoderType,
};

/// Encoder trait
///
/// All encoders must be Send + 'static as they run on dedicated threads.
/// The encoder receives [`crate::media::CapturedFrame`] from the capture thread and outputs
/// EncodedPacket via the channel returned by packet_rx().
pub trait Encoder: Send + 'static {
    /// Initialize encoder with configuration
    fn init(&mut self, config: &ResolvedEncoderConfig) -> EncodeResult<()>;
    /// Encode a frame
    fn encode_frame(&mut self, frame: &crate::media::CapturedFrame) -> EncodeResult<()>;
    /// Flush encoder and get remaining packets
    fn flush(&mut self) -> EncodeResult<Vec<EncodedPacket>>;
    /// Get receiver for encoded packets
    fn packet_rx(&self) -> Receiver<EncodedPacket>;
    /// Check if encoder is still running
    fn is_running(&self) -> bool;
}

/// Factory trait for spawning encoder instances.
///
/// This abstraction allows dependency injection for testing and
/// supports alternative encoder backends.
///
/// # Thread Safety
///
/// Implementations must be `Send + Sync` to allow sharing across threads.
pub trait EncoderFactory: Send + Sync + 'static {
    /// Spawn an encoder that receives frames from the given receiver.
    ///
    /// `config` must already be resolved (no [`crate::config::EncoderType::Auto`]).
    fn spawn(
        &self,
        config: ResolvedEncoderConfig,
        buffer: crate::buffer::ring::SharedReplayBuffer,
        frame_rx: Receiver<crate::media::CapturedFrame>,
    ) -> EncodeResult<EncoderHandle>;
}

/// Default encoder factory using FFmpeg.
///
/// This factory creates encoders based on the configuration,
/// with automatic hardware detection when `Auto` is selected.
pub struct DefaultEncoderFactory;

impl EncoderFactory for DefaultEncoderFactory {
    fn spawn(
        &self,
        config: ResolvedEncoderConfig,
        buffer: crate::buffer::ring::SharedReplayBuffer,
        frame_rx: Receiver<crate::media::CapturedFrame>,
    ) -> EncodeResult<EncoderHandle> {
        spawn_encoder_with_receiver(config, buffer, frame_rx)
    }
}

// Hardware encoder registry: when changing NVENC/QSV/AMF codec strings, probe options, or auto-detect
// order, see the contributor hub in `encode::ffmpeg` and `ResolvedEncoderConfig` / `ResolvedEncoderType`.
#[cfg(feature = "ffmpeg")]
fn ensure_requested_encoder_available(ty: ResolvedEncoderType) -> EncodeResult<()> {
    let codec_name = ty.ffmpeg_hevc_codec_name();
    if !probe_encoder_available(codec_name) {
        return Err(EncodeError::EncoderUnavailable { encoder: ty.into() });
    }
    Ok(())
}

#[cfg(not(feature = "ffmpeg"))]
fn ensure_requested_encoder_available(_ty: ResolvedEncoderType) -> EncodeResult<()> {
    Ok(())
}

fn encoder_fields_to_resolved(c: EncoderConfig, ty: ResolvedEncoderType) -> ResolvedEncoderConfig {
    ResolvedEncoderConfig {
        bitrate_mbps: c.bitrate_mbps,
        framerate: c.framerate,
        resolution: c.resolution,
        use_native_resolution: c.use_native_resolution,
        encoder_type: ty,
        quality_preset: c.quality_preset,
        rate_control: c.rate_control,
        quality_value: c.quality_value,
        keyframe_interval_secs: c.keyframe_interval_secs,
        use_cpu_readback: c.use_cpu_readback,
        output_index: c.output_index,
    }
}

fn resolve_encoder_config(config: &EncoderConfig) -> EncodeResult<ResolvedEncoderConfig> {
    let mut resolved = config.clone();
    if resolved.encoder_type == crate::config::EncoderType::Auto {
        let detected_encoder = detect_hardware_encoder();
        if matches!(detected_encoder, HardwareEncoder::None) {
            // Fallback to software encoder when no hardware encoder is available.
            // This ensures the app is usable on any system, even without NVENC/AMF/QSV.
            info!("No hardware encoder detected, falling back to software encoder (libx265)");
            resolved.encoder_type = crate::config::EncoderType::Software;
        } else {
            apply_auto_encoder_selection(&mut resolved, detected_encoder);
            info!("Auto-detected encoder: {:?}", resolved.encoder_type);
        }
    }

    let ty = match resolved.encoder_type {
        crate::config::EncoderType::Nvenc => ResolvedEncoderType::Nvenc,
        crate::config::EncoderType::Amf => ResolvedEncoderType::Amf,
        crate::config::EncoderType::Qsv => ResolvedEncoderType::Qsv,
        crate::config::EncoderType::Software => ResolvedEncoderType::Software,
        crate::config::EncoderType::Auto => {
            // This should never happen after the Auto handling above,
            // but provide a safe fallback anyway.
            ResolvedEncoderType::Software
        }
    };

    ensure_requested_encoder_available(ty)?;
    Ok(encoder_fields_to_resolved(resolved, ty))
}

pub fn resolve_effective_encoder_config(
    config: &EncoderConfig,
) -> EncodeResult<ResolvedEncoderConfig> {
    resolve_encoder_config(config)
}

fn set_encoder_thread_priority() {
    #[cfg(windows)]
    {
        use windows::Win32::System::Threading::THREAD_PRIORITY_BELOW_NORMAL;
        unsafe {
            // Use BELOW_NORMAL priority to ensure encoder never competes with game
            if let Err(e) = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL) {
                warn!("Failed to set encoder thread priority: {}", e);
            } else {
                debug!("Encoder thread priority set to BELOW_NORMAL");
            }
        }
    }
}

// Minimal open probe per codec; NVENC/QSV/AMF arms should stay consistent with `encode/ffmpeg/options.rs`.
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
///
/// Changing this order or codec names requires updating the contributor checklist in `encode::ffmpeg`.
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

#[cfg(feature = "ffmpeg")]
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

/// Create the FFmpeg encoder for a **resolved** configuration (after `Auto` handling).
#[cfg(feature = "ffmpeg")]
pub fn create_encoder(config: &ResolvedEncoderConfig) -> EncodeResult<Box<dyn Encoder>> {
    info!("Creating native FFmpeg encoder: {:?}", config.encoder_type);
    Ok(Box::new(crate::encode::ffmpeg::FfmpegEncoder::new(config)?))
}

#[cfg(not(feature = "ffmpeg"))]
pub fn create_encoder(_config: &ResolvedEncoderConfig) -> EncodeResult<Box<dyn Encoder>> {
    Err(EncodeError::msg(
        "FFmpeg support is disabled; rebuild with `--features ffmpeg`",
    ))
}

/// Spawn an encoder that receives frames from an existing receiver
///
/// This is used when the capture provides its own frame channel.
/// The encoder thread reads frames from frame_rx and pushes encoded packets
/// directly to the SharedReplayBuffer.
pub fn spawn_encoder_with_receiver(
    effective_config: ResolvedEncoderConfig,
    buffer: crate::buffer::ring::SharedReplayBuffer,
    frame_rx: Receiver<crate::media::CapturedFrame>,
) -> EncodeResult<EncoderHandle> {
    const MAX_CONSECUTIVE_ENCODE_ERRORS: u32 = 8;
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
            let mut packet_batch = Vec::with_capacity(256);
            let mut consecutive_encode_errors = 0u32;
            const MAX_FRAME_BURST: usize = 8;
            const MAX_PACKET_BATCH_LEN: usize = 32;
            const MAX_PACKET_BATCH_AGE_MS: u128 = 75;
            let mut total_forwarded_packets = 0u64;
            let mut flush_batches = 0usize;
            let mut last_packet_flush = Instant::now();
            // Wake at most at the batch-flush granularity when no frames arrive.
            // drain_ready_packets is called inline after every successful encode_frame,
            // so this timeout only needs to cover flushing stale packet batches.
            // Dropping from ~8-16 ms (1000/fps) down to batch-age cuts idle wakeups
            // from 60-125/sec to ~13/sec with zero latency impact.
            let frame_recv_timeout = Duration::from_millis(MAX_PACKET_BATCH_AGE_MS as u64);

            fn flush_packet_batch(
                buffer: &crate::buffer::ring::SharedReplayBuffer,
                packet_batch: &mut Vec<EncodedPacket>,
                flush_batches: &mut usize,
                last_packet_flush: &mut Instant,
            ) {
                if packet_batch.is_empty() {
                    return;
                }
                buffer.push_batch(packet_batch.drain(..));
                *flush_batches += 1;
                *last_packet_flush = Instant::now();
            }

            fn drain_ready_packets(
                packet_rx: &Receiver<EncodedPacket>,
                buffer: &crate::buffer::ring::SharedReplayBuffer,
                packet_batch: &mut Vec<EncodedPacket>,
                flush_batches: &mut usize,
                last_packet_flush: &mut Instant,
            ) -> u64 {
                let mut drained = 0u64;
                while let Ok(packet) = packet_rx.try_recv() {
                    packet_batch.push(packet);
                    drained = drained.saturating_add(1);
                    if packet_batch.len() >= MAX_PACKET_BATCH_LEN {
                        flush_packet_batch(buffer, packet_batch, flush_batches, last_packet_flush);
                    }
                }
                if !packet_batch.is_empty()
                    && last_packet_flush.elapsed().as_millis() >= MAX_PACKET_BATCH_AGE_MS
                {
                    flush_packet_batch(buffer, packet_batch, flush_batches, last_packet_flush);
                }
                drained
            }

            loop {
                total_forwarded_packets =
                    total_forwarded_packets.saturating_add(drain_ready_packets(
                        &packet_rx,
                        &buffer,
                        &mut packet_batch,
                        &mut flush_batches,
                        &mut last_packet_flush,
                    ));
                match frame_rx.recv_timeout(frame_recv_timeout) {
                    Ok(frame) => {
                        let mut encode_one =
                            |frame: crate::media::CapturedFrame| -> EncodeResult<()> {
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
                                        return Err(EncodeError::msg(reason));
                                    }
                                } else {
                                    consecutive_encode_errors = 0;
                                }
                                total_forwarded_packets =
                                    total_forwarded_packets.saturating_add(drain_ready_packets(
                                        &packet_rx,
                                        &buffer,
                                        &mut packet_batch,
                                        &mut flush_batches,
                                        &mut last_packet_flush,
                                    ));
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
                        total_forwarded_packets =
                            total_forwarded_packets.saturating_add(drain_ready_packets(
                                &packet_rx,
                                &buffer,
                                &mut packet_batch,
                                &mut flush_batches,
                                &mut last_packet_flush,
                            ));
                        flush_packet_batch(
                            &buffer,
                            &mut packet_batch,
                            &mut flush_batches,
                            &mut last_packet_flush,
                        );
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
                        packet_batch.push(packet);
                        if packet_batch.len() >= MAX_PACKET_BATCH_LEN {
                            flush_packet_batch(
                                &buffer,
                                &mut packet_batch,
                                &mut flush_batches,
                                &mut last_packet_flush,
                            );
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to flush encoder: {}", e);
                }
            }
            total_forwarded_packets = total_forwarded_packets.saturating_add(drain_ready_packets(
                &packet_rx,
                &buffer,
                &mut packet_batch,
                &mut flush_batches,
                &mut last_packet_flush,
            ));
            flush_packet_batch(
                &buffer,
                &mut packet_batch,
                &mut flush_batches,
                &mut last_packet_flush,
            );
            debug!(
                "Encoder buffer flush complete: {} packets across {} batches",
                total_forwarded_packets, flush_batches
            );
            debug!("Encoder thread stopped");
            Ok(())
        })
        .map_err(EncodeError::Io)?;
    let (dummy_frame_tx, _) = bounded(1);
    let handle = EncoderHandle {
        thread,
        frame_tx: dummy_frame_tx,
        health_rx,
        effective_config,
    };
    Ok(handle)
}

/// Initialize FFmpeg (call once at startup)
#[cfg(feature = "ffmpeg")]
pub fn init_ffmpeg() -> EncodeResult<()> {
    ffmpeg_next::init()
        .map_err(|e| EncodeError::msg(format!("Failed to initialize FFmpeg: {}", e)))?;
    info!("FFmpeg initialized successfully");
    Ok(())
}

/// Initialize FFmpeg (call once at startup)
#[cfg(not(feature = "ffmpeg"))]
pub fn init_ffmpeg() -> EncodeResult<()> {
    info!("FFmpeg initialization skipped (not compiled in)");
    Ok(())
}

impl From<HardwareEncoder> for crate::config::EncoderType {
    fn from(hw: HardwareEncoder) -> Self {
        match hw {
            HardwareEncoder::Nvenc => crate::config::EncoderType::Nvenc,
            HardwareEncoder::Amf => crate::config::EncoderType::Amf,
            HardwareEncoder::Qsv => crate::config::EncoderType::Qsv,
            HardwareEncoder::None => crate::config::EncoderType::Auto,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode::EncoderConfig;

    /// Tests that when Auto encoder is selected and no hardware encoder is available,
    /// the system falls back to Software encoder instead of returning an error.
    /// This ensures the app is usable on any system, even without NVENC/AMF/QSV.
    #[test]
    fn test_auto_encoder_fallback_to_software_when_no_hardware() {
        // Create a config with Auto encoder type
        let config = EncoderConfig::new(
            25, // bitrate_mbps
            60, // framerate
            (1920, 1080),
            crate::config::EncoderType::Auto,
            2, // keyframe_interval_secs
        );

        // Check if hardware encoder is detected
        let hw_encoder = detect_hardware_encoder();

        // This should succeed - either using detected hardware encoder or falling back to Software
        let result = resolve_effective_encoder_config(&config);

        // Should NOT return NoHardwareForAuto error anymore
        match result {
            Ok(resolved) => {
                // If hardware encoder was available, use it
                // If not, fallback to Software
                match hw_encoder {
                    HardwareEncoder::None => {
                        assert_eq!(
                            resolved.encoder_type,
                            ResolvedEncoderType::Software,
                            "Auto encoder should fallback to Software when no hardware encoder available"
                        );
                    }
                    HardwareEncoder::Nvenc => {
                        assert_eq!(
                            resolved.encoder_type,
                            ResolvedEncoderType::Nvenc,
                            "Auto encoder should use NVENC when available"
                        );
                    }
                    HardwareEncoder::Amf => {
                        assert_eq!(
                            resolved.encoder_type,
                            ResolvedEncoderType::Amf,
                            "Auto encoder should use AMF when available"
                        );
                    }
                    HardwareEncoder::Qsv => {
                        assert_eq!(
                            resolved.encoder_type,
                            ResolvedEncoderType::Qsv,
                            "Auto encoder should use QSV when available"
                        );
                    }
                }
            }
            Err(e) => {
                // Should not happen - even without hardware, we should fallback to Software
                panic!("resolve_effective_encoder_config should not fail: {}", e);
            }
        }
    }

    /// Tests that explicitly selecting Software encoder works correctly.
    #[test]
    fn test_explicit_software_encoder_selection() {
        let config = EncoderConfig::new(
            25,
            60,
            (1920, 1080),
            crate::config::EncoderType::Software,
            2,
        );

        let result = resolve_effective_encoder_config(&config);
        assert!(
            result.is_ok(),
            "Explicit Software encoder selection should work"
        );

        let resolved = result.unwrap();
        assert_eq!(
            resolved.encoder_type,
            ResolvedEncoderType::Software,
            "Resolved type should be Software"
        );
        assert!(
            !resolved.supports_gpu_frame_transport(),
            "Software encoder should not support GPU frame transport"
        );
        assert_eq!(
            resolved.ffmpeg_codec_name(),
            "libx265",
            "Software encoder should use libx265 codec"
        );
    }

    /// Tests that ResolvedEncoderType::Software codec name is libx265.
    #[test]
    fn test_software_encoder_codec_name() {
        assert_eq!(
            ResolvedEncoderType::Software.ffmpeg_hevc_codec_name(),
            "libx265"
        );
    }
}
