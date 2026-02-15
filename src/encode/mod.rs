//! Encode Pipeline
//!
//! Encodes raw D3D11 textures into H.264/H.265/AV1 using hardware acceleration.
//! Supports NVENC (NVIDIA), AMF (AMD), QSV (Intel), and libx264 software fallback.

use anyhow::Result;
use bytes::Bytes;
pub use crossbeam::channel::Receiver as CrossbeamReceiver;
use crossbeam::channel::{bounded, Receiver, Sender};
use tracing::{debug, info, warn};

pub mod cpu_readback;
pub mod hw_encoder;
pub mod sw_encoder;

/// Encoder configuration
#[derive(Debug, Clone)]
pub struct EncoderConfig {
    /// Video codec to use
    pub codec: crate::config::Codec,
    /// Target bitrate in Mbps
    pub bitrate_mbps: u32,
    /// Target framerate
    pub framerate: u32,
    /// Output resolution (width, height)
    pub resolution: (u32, u32),
    /// Whether to use native capture resolution (ignores resolution field)
    pub use_native_resolution: bool,
    /// Encoder type selection
    pub encoder_type: crate::config::EncoderType,
    /// Keyframe interval in seconds
    pub keyframe_interval_secs: u32,
    /// Force CPU readback path (Phase 1 fallback)
    pub use_cpu_readback: bool,
}

impl From<&crate::config::Config> for EncoderConfig {
    fn from(config: &crate::config::Config) -> Self {
        let use_native_resolution =
            matches!(config.video.resolution, crate::config::Resolution::Native);
        Self {
            codec: config.video.codec,
            bitrate_mbps: config.video.bitrate_mbps,
            framerate: config.video.framerate,
            resolution: match config.video.resolution {
                crate::config::Resolution::Native => (0, 0), // Will be set from captured frame
                crate::config::Resolution::P1080 => (1920, 1080),
                crate::config::Resolution::P720 => (1280, 720),
                crate::config::Resolution::P480 => (854, 480),
            },
            use_native_resolution,
            encoder_type: config.video.encoder,
            keyframe_interval_secs: config.advanced.keyframe_interval_secs,
            use_cpu_readback: config.advanced.use_cpu_readback,
        }
    }
}

impl EncoderConfig {
    /// Create encoder configuration with explicit parameters
    pub fn new(
        codec: crate::config::Codec,
        bitrate_mbps: u32,
        framerate: u32,
        resolution: (u32, u32),
        encoder_type: crate::config::EncoderType,
        keyframe_interval_secs: u32,
    ) -> Self {
        Self {
            codec,
            bitrate_mbps,
            framerate,
            resolution,
            use_native_resolution: false,
            encoder_type,
            keyframe_interval_secs,
            use_cpu_readback: false,
        }
    }

    /// Get the FFmpeg codec name based on codec and encoder type
    pub fn ffmpeg_codec_name(&self) -> &'static str {
        match (self.codec, self.encoder_type) {
            // H.264
            (crate::config::Codec::H264, crate::config::EncoderType::Nvenc) => "h264_nvenc",
            (crate::config::Codec::H264, crate::config::EncoderType::Amf) => "h264_amf",
            (crate::config::Codec::H264, crate::config::EncoderType::Qsv) => "h264_qsv",
            (crate::config::Codec::H264, _) => "libx264",
            // H.265/HEVC
            (crate::config::Codec::H265, crate::config::EncoderType::Nvenc) => "hevc_nvenc",
            (crate::config::Codec::H265, crate::config::EncoderType::Amf) => "hevc_amf",
            (crate::config::Codec::H265, crate::config::EncoderType::Qsv) => "hevc_qsv",
            (crate::config::Codec::H265, _) => "libx265",
            // AV1
            (crate::config::Codec::Av1, crate::config::EncoderType::Nvenc) => "av1_nvenc",
            (_, _) => "libaom-av1",
        }
    }

    /// Calculate keyframe interval in frames
    pub fn keyframe_interval_frames(&self) -> u32 {
        self.keyframe_interval_secs * self.framerate
    }
}

/// Encoded packet data
///
/// Uses `Bytes` for reference-counted data, making clones cheap (just a ref count bump).
/// This is critical for the snapshot operation which needs to quickly clone the entire
/// buffer when saving clips.
#[derive(Clone)]
pub struct EncodedPacket {
    /// Reference-counted byte buffer (cheap clone)
    pub data: Bytes,
    /// Presentation timestamp (QPC-based, 10MHz units)
    pub pts: i64,
    /// Decode timestamp
    pub dts: i64,
    /// True if this is a keyframe (IDR frame)
    pub is_keyframe: bool,
    /// Stream type
    pub stream: StreamType,
    /// Optional frame resolution for raw video payloads
    pub resolution: Option<(u32, u32)>,
}

/// Stream type for multiplexed output
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamType {
    /// Video stream (H.264/H.265/AV1)
    Video,
    /// System audio (game/desktop audio)
    SystemAudio,
    /// Microphone input
    Microphone,
}

impl std::fmt::Debug for EncodedPacket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EncodedPacket")
            .field("size", &self.data.len())
            .field("pts", &self.pts)
            .field("dts", &self.dts)
            .field("is_keyframe", &self.is_keyframe)
            .field("stream", &self.stream)
            .field("resolution", &self.resolution)
            .finish()
    }
}

impl EncodedPacket {
    /// Create a new encoded packet
    pub fn new(
        data: impl Into<Bytes>,
        pts: i64,
        dts: i64,
        is_keyframe: bool,
        stream: StreamType,
    ) -> Self {
        Self {
            data: data.into(),
            pts,
            dts,
            is_keyframe,
            stream,
            resolution: None,
        }
    }

    /// Create a video keyframe packet
    pub fn video_keyframe(data: impl Into<Bytes>, pts: i64) -> Self {
        Self::new(data, pts, pts, true, StreamType::Video)
    }

    /// Create a video delta frame packet
    pub fn video_delta(data: impl Into<Bytes>, pts: i64) -> Self {
        Self::new(data, pts, pts, false, StreamType::Video)
    }
}

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

/// Hardware detection result
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardwareEncoder {
    /// NVIDIA NVENC available
    Nvenc,
    /// AMD AMF available
    Amf,
    /// Intel QSV available
    Qsv,
    /// No hardware encoder available
    None,
}

/// Detect available hardware encoder
///
/// Priority order: NVENC → AMF → QSV → None (software)
#[cfg(feature = "ffmpeg")]
pub fn detect_hardware_encoder() -> HardwareEncoder {
    // Try to detect NVENC first
    if hw_encoder::check_encoder_available("h264_nvenc") {
        info!("Detected NVIDIA NVENC encoder");
        return HardwareEncoder::Nvenc;
    }

    // Try AMF (AMD)
    if hw_encoder::check_encoder_available("h264_amf") {
        info!("Detected AMD AMF encoder");
        return HardwareEncoder::Amf;
    }

    // Try QSV (Intel)
    if hw_encoder::check_encoder_available("h264_qsv") {
        info!("Detected Intel QSV encoder");
        return HardwareEncoder::Qsv;
    }

    info!("No hardware encoder detected, using software encoding");
    HardwareEncoder::None
}

/// Detect available hardware encoder (non-FFmpeg fallback)
#[cfg(not(feature = "ffmpeg"))]
pub fn detect_hardware_encoder() -> HardwareEncoder {
    // Without FFmpeg, hardware encoders are not available
    info!("FFmpeg not compiled in, using software encoding");
    HardwareEncoder::None
}

/// Check if a specific FFmpeg encoder is available
#[allow(dead_code)]
fn is_encoder_available(codec_name: &str) -> bool {
    // FFmpeg not available in this environment
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

    match encoder_type {
        crate::config::EncoderType::Auto => {
            // Try hardware encoders in priority order: NVENC -> AMF -> QSV
            let hw = detect_hardware_encoder();
            match hw {
                HardwareEncoder::Nvenc => match hw_encoder::NvencEncoder::new(config) {
                    Ok(enc) => {
                        info!("Using NVENC encoder");
                        return Ok(Box::new(enc) as Box<dyn Encoder>);
                    }
                    Err(e) => warn!("Failed to create NVENC encoder: {}", e),
                },
                HardwareEncoder::Amf => match hw_encoder::AmfEncoder::new(config) {
                    Ok(enc) => {
                        info!("Using AMF encoder");
                        return Ok(Box::new(enc) as Box<dyn Encoder>);
                    }
                    Err(e) => warn!("Failed to create AMF encoder: {}", e),
                },
                HardwareEncoder::Qsv => match hw_encoder::QsvEncoder::new(config) {
                    Ok(enc) => {
                        info!("Using QSV encoder");
                        return Ok(Box::new(enc) as Box<dyn Encoder>);
                    }
                    Err(e) => warn!("Failed to create QSV encoder: {}", e),
                },
                HardwareEncoder::None => {}
            }
            // Fall back to software
            info!("No hardware encoder available, using software encoder");
            sw_encoder::SoftwareEncoder::new(config).map(|e| Box::new(e) as Box<dyn Encoder>)
        }
        crate::config::EncoderType::Nvenc => {
            match hw_encoder::NvencEncoder::new(config) {
                Ok(enc) => {
                    info!("Using NVENC encoder");
                    return Ok(Box::new(enc) as Box<dyn Encoder>);
                }
                Err(e) => warn!(
                    "Failed to create NVENC encoder: {}, falling back to software",
                    e
                ),
            }
            sw_encoder::SoftwareEncoder::new(config).map(|e| Box::new(e) as Box<dyn Encoder>)
        }
        crate::config::EncoderType::Amf => {
            match hw_encoder::AmfEncoder::new(config) {
                Ok(enc) => {
                    info!("Using AMF encoder");
                    return Ok(Box::new(enc) as Box<dyn Encoder>);
                }
                Err(e) => warn!(
                    "Failed to create AMF encoder: {}, falling back to software",
                    e
                ),
            }
            sw_encoder::SoftwareEncoder::new(config).map(|e| Box::new(e) as Box<dyn Encoder>)
        }
        crate::config::EncoderType::Qsv => {
            match hw_encoder::QsvEncoder::new(config) {
                Ok(enc) => {
                    info!("Using QSV encoder");
                    return Ok(Box::new(enc) as Box<dyn Encoder>);
                }
                Err(e) => warn!(
                    "Failed to create QSV encoder: {}, falling back to software",
                    e
                ),
            }
            sw_encoder::SoftwareEncoder::new(config).map(|e| Box::new(e) as Box<dyn Encoder>)
        }
        crate::config::EncoderType::Software => {
            info!("Using software encoder");
            sw_encoder::SoftwareEncoder::new(config).map(|e| Box::new(e) as Box<dyn Encoder>)
        }
    }
}

/// Encoder thread handle
pub struct EncoderHandle {
    /// Join handle for the encoder thread
    pub thread: std::thread::JoinHandle<Result<()>>,
    /// Channel sender for frames
    pub frame_tx: Sender<crate::capture::CapturedFrame>,
    /// Channel receiver for packets
    pub packet_rx: Receiver<EncodedPacket>,
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
    let (frame_tx, frame_rx): (Sender<crate::capture::CapturedFrame>, _) = bounded(32);

    let frame_tx_clone = frame_tx.clone();

    let thread = std::thread::spawn(move || {
        info!("Encoder thread started");

        // Create and initialize encoder
        let mut encoder = create_encoder(&config)?;
        encoder.init(&config)?;

        info!("Encoder initialized and ready");

        // Get the packet receiver from the encoder
        let packet_rx = encoder.packet_rx();

        // Main encode loop – dispatch every frame to parallel workers
        // and drain completed packets into the ring buffer.
        loop {
            // First, drain any already-encoded packets into the buffer
            while let Ok(packet) = packet_rx.try_recv() {
                buffer.push(packet);
            }

            match frame_rx.recv() {
                Ok(frame) => {
                    if let Err(e) = encoder.encode_frame(&frame) {
                        warn!("Failed to encode frame: {}", e);
                    }
                }
                Err(_) => {
                    debug!("Frame channel closed, shutting down encoder");
                    break;
                }
            }
        }

        // Flush remaining packets
        info!("Encoder thread shutting down, flushing remaining packets");
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

        // Final drain of worker output
        while let Ok(packet) = packet_rx.try_recv() {
            buffer.push(packet);
        }

        info!("Encoder thread stopped");
        Ok(())
    });

    // Create a dummy packet_rx for the handle (not used directly since we push to buffer)
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
    let thread = std::thread::spawn(move || {
        info!("Encoder thread started");

        // Create and initialize encoder
        let mut encoder = create_encoder(&config)?;
        encoder.init(&config)?;

        info!("Encoder initialized and ready");

        // Get the packet receiver from the encoder
        let packet_rx = encoder.packet_rx();

        // Main encode loop – dispatch every frame to parallel workers
        // and drain completed packets into the ring buffer.
        loop {
            // Drain completed packets into the buffer
            while let Ok(packet) = packet_rx.try_recv() {
                buffer.push(packet);
            }

            match frame_rx.recv() {
                Ok(frame) => {
                    if let Err(e) = encoder.encode_frame(&frame) {
                        warn!("Failed to encode frame: {}", e);
                    }
                }
                Err(_) => {
                    debug!("Frame channel closed, shutting down encoder");
                    break;
                }
            }
        }

        // Flush remaining packets
        info!("Encoder thread shutting down, flushing remaining packets");
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

        // Final drain of worker output
        while let Ok(packet) = packet_rx.try_recv() {
            buffer.push(packet);
        }

        info!("Encoder thread stopped");
        Ok(())
    });

    // Create a dummy packet_rx for the handle (not used directly since we push to buffer)
    let (_, dummy_packet_rx) = bounded(1);

    // Create a dummy frame_tx (not used since we receive from capture's channel)
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
    // FFmpeg not available in this environment
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encoder_config_codec_names() {
        // Test H264 with different encoders
        let mut config = EncoderConfig::new(
            crate::config::Codec::H264,
            20,
            30,
            (1920, 1080),
            crate::config::EncoderType::Nvenc,
            1,
        );
        assert_eq!(config.ffmpeg_codec_name(), "h264_nvenc");

        config.encoder_type = crate::config::EncoderType::Amf;
        assert_eq!(config.ffmpeg_codec_name(), "h264_amf");

        config.encoder_type = crate::config::EncoderType::Qsv;
        assert_eq!(config.ffmpeg_codec_name(), "h264_qsv");

        config.encoder_type = crate::config::EncoderType::Software;
        assert_eq!(config.ffmpeg_codec_name(), "libx264");
    }

    #[test]
    fn test_keyframe_interval_calculation() {
        let config = EncoderConfig::new(
            crate::config::Codec::H264,
            20,
            30,
            (1920, 1080),
            crate::config::EncoderType::Nvenc,
            2,
        );
        assert_eq!(config.keyframe_interval_frames(), 60); // 2 seconds * 30fps
    }

    #[test]
    fn test_encoded_packet_creation() {
        let packet = EncodedPacket::video_keyframe(vec![0u8; 1024], 1_000_000);
        assert_eq!(packet.data.len(), 1024);
        assert_eq!(packet.pts, 1_000_000);
        assert!(packet.is_keyframe);
        assert!(matches!(packet.stream, StreamType::Video));

        let packet = EncodedPacket::video_delta(vec![0u8; 512], 2_000_000);
        assert!(!packet.is_keyframe);
    }

    #[test]
    fn test_hardware_encoder_conversion() {
        assert!(matches!(
            HardwareEncoder::Nvenc.into(),
            crate::config::EncoderType::Nvenc
        ));
        assert!(matches!(
            HardwareEncoder::Amf.into(),
            crate::config::EncoderType::Amf
        ));
        assert!(matches!(
            HardwareEncoder::Qsv.into(),
            crate::config::EncoderType::Qsv
        ));
        assert!(matches!(
            HardwareEncoder::None.into(),
            crate::config::EncoderType::Software
        ));
    }
}
