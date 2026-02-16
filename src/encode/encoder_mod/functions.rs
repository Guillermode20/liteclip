//! Auto-generated module
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use crate::encode::{hw_encoder, sw_encoder};
use anyhow::Result;
use crossbeam::channel::{bounded, Receiver, Sender};
use tracing::{debug, info, warn};

use super::types::{EncodedPacket, EncoderConfig, EncoderHandle, HardwareEncoder};
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
/// Detect available hardware encoder
///
/// Priority order: NVENC → AMF → QSV → None (software)
#[cfg(feature = "ffmpeg")]
pub fn detect_hardware_encoder() -> HardwareEncoder {
    debug!("Detecting hardware encoders...");
    if hw_encoder::check_encoder_available("h264_nvenc") {
        info!("Using NVIDIA NVENC encoder");
        return HardwareEncoder::Nvenc;
    }
    if hw_encoder::check_encoder_available("h264_amf") {
        info!("Using AMD AMF encoder");
        return HardwareEncoder::Amf;
    }
    if hw_encoder::check_encoder_available("h264_qsv") {
        info!("Using Intel QSV encoder");
        return HardwareEncoder::Qsv;
    }
    info!("No hardware encoder, using software encoding");
    HardwareEncoder::None
}
/// Detect available hardware encoder (non-FFmpeg fallback)
#[cfg(not(feature = "ffmpeg"))]
pub fn detect_hardware_encoder() -> HardwareEncoder {
    info!("FFmpeg not compiled in, using software encoding");
    HardwareEncoder::None
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
    let encoder_type = config.encoder_type;
    let encoder: Box<dyn Encoder> = match encoder_type {
        crate::config::EncoderType::Auto => {
            let hw = detect_hardware_encoder();
            match hw {
                HardwareEncoder::Nvenc => {
                    if let Ok(enc) = hw_encoder::NvencEncoder::new(config) {
                        return Ok(Box::new(enc) as Box<dyn Encoder>);
                    }
                }
                HardwareEncoder::Amf => {
                    if let Ok(enc) = hw_encoder::AmfEncoder::new(config) {
                        return Ok(Box::new(enc) as Box<dyn Encoder>);
                    }
                }
                HardwareEncoder::Qsv => {
                    if let Ok(enc) = hw_encoder::QsvEncoder::new(config) {
                        return Ok(Box::new(enc) as Box<dyn Encoder>);
                    }
                }
                HardwareEncoder::None => {}
            }
            info!("No hardware encoder available, using software encoder");
            Box::new(sw_encoder::SoftwareEncoder::new(config)?) as Box<dyn Encoder>
        }
        crate::config::EncoderType::Nvenc => match hw_encoder::NvencEncoder::new(config) {
            Ok(enc) => Box::new(enc) as Box<dyn Encoder>,
            Err(e) => {
                warn!(
                    "Failed to create NVENC encoder: {}, falling back to software",
                    e
                );
                Box::new(sw_encoder::SoftwareEncoder::new(config)?) as Box<dyn Encoder>
            }
        },
        crate::config::EncoderType::Amf => match hw_encoder::AmfEncoder::new(config) {
            Ok(enc) => Box::new(enc) as Box<dyn Encoder>,
            Err(e) => {
                warn!(
                    "Failed to create AMF encoder: {}, falling back to software",
                    e
                );
                Box::new(sw_encoder::SoftwareEncoder::new(config)?) as Box<dyn Encoder>
            }
        },
        crate::config::EncoderType::Qsv => match hw_encoder::QsvEncoder::new(config) {
            Ok(enc) => Box::new(enc) as Box<dyn Encoder>,
            Err(e) => {
                warn!(
                    "Failed to create QSV encoder: {}, falling back to software",
                    e
                );
                Box::new(sw_encoder::SoftwareEncoder::new(config)?) as Box<dyn Encoder>
            }
        },
        crate::config::EncoderType::Software => {
            Box::new(sw_encoder::SoftwareEncoder::new(config)?) as Box<dyn Encoder>
        }
    };
    Ok(encoder)
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
    let (frame_tx, frame_rx): (Sender<crate::capture::CapturedFrame>, _) = bounded(64);
    let frame_tx_clone = frame_tx.clone();
    let thread = std::thread::Builder::new()
        .name("encoder".to_string())
        .stack_size(4 * 1024 * 1024)
        .spawn(move || {
            debug!("Encoder thread started");
            let mut encoder = create_encoder(&config)?;
            encoder.init(&config)?;
            debug!("Encoder initialized");
            let packet_rx = encoder.packet_rx();
            let mut packet_batch = Vec::with_capacity(16);
            loop {
                let mut count = 0;
                while let Ok(packet) = packet_rx.try_recv() {
                    packet_batch.push(packet);
                    count += 1;
                    if count >= 16 {
                        break;
                    }
                }
                if !packet_batch.is_empty() {
                    for packet in packet_batch.drain(..) {
                        buffer.push(packet);
                    }
                }
                match frame_rx.recv_timeout(std::time::Duration::from_millis(10)) {
                    Ok(frame) => {
                        if let Err(e) = encoder.encode_frame(&frame) {
                            warn!("Failed to encode frame: {}", e);
                        }
                    }
                    Err(crossbeam::channel::RecvTimeoutError::Timeout) => {}
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
                        buffer.push(packet);
                    }
                }
                Err(e) => {
                    warn!("Failed to flush encoder: {}", e);
                }
            }
            let mut final_packets = Vec::new();
            while let Ok(packet) = packet_rx.try_recv() {
                final_packets.push(packet);
            }
            for packet in final_packets {
                buffer.push(packet);
            }
            debug!("Encoder thread stopped");
            Ok(())
        })?;
    let (_, dummy_packet_rx) = bounded(1);
    let handle = EncoderHandle {
        thread,
        frame_tx: frame_tx_clone.clone(),
        packet_rx: dummy_packet_rx,
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
    let thread = std::thread::Builder::new()
        .name("encoder".to_string())
        .stack_size(4 * 1024 * 1024)
        .spawn(move || {
            debug!("Encoder thread started");
            let mut encoder = create_encoder(&config)?;
            encoder.init(&config)?;
            debug!("Encoder initialized");
            let packet_rx = encoder.packet_rx();
            let mut packet_batch = Vec::with_capacity(16);
            loop {
                let mut count = 0;
                while let Ok(packet) = packet_rx.try_recv() {
                    packet_batch.push(packet);
                    count += 1;
                    if count >= 16 {
                        break;
                    }
                }
                if !packet_batch.is_empty() {
                    for packet in packet_batch.drain(..) {
                        buffer.push(packet);
                    }
                }
                match frame_rx.recv_timeout(std::time::Duration::from_millis(10)) {
                    Ok(frame) => {
                        if let Err(e) = encoder.encode_frame(&frame) {
                            warn!("Failed to encode frame: {}", e);
                        }
                    }
                    Err(crossbeam::channel::RecvTimeoutError::Timeout) => {}
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
                        buffer.push(packet);
                    }
                }
                Err(e) => {
                    warn!("Failed to flush encoder: {}", e);
                }
            }
            let mut final_packets = Vec::new();
            while let Ok(packet) = packet_rx.try_recv() {
                final_packets.push(packet);
            }
            for packet in final_packets {
                buffer.push(packet);
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
    };
    Ok(handle)
}
/// Initialize FFmpeg (call once at startup)
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
            HardwareEncoder::None => crate::config::EncoderType::Software,
        }
    }
}
