//! Auto-generated module
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use anyhow::Result;
use bytes::Bytes;
use crossbeam::channel::{Receiver, Sender};

#[derive(Debug, Clone)]
pub enum EncoderHealthEvent {
    Fatal(String),
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
/// Stream type for multiplexed output
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamType {
    /// Video stream (HEVC)
    Video,
    /// System audio (game/desktop audio)
    SystemAudio,
    /// Microphone input
    Microphone,
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
/// Encoder configuration (HEVC-only, hardware encoders)
#[derive(Debug, Clone)]
pub struct EncoderConfig {
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
    /// Quality preset preference (speed vs quality)
    pub quality_preset: crate::config::QualityPreset,
    /// Rate control mode preference
    pub rate_control: crate::config::RateControl,
    /// Optional quality scalar (e.g. CQ/CRF-like)
    pub quality_value: Option<u8>,
    /// Keyframe interval in seconds
    pub keyframe_interval_secs: u32,
    /// Force CPU readback path (Phase 1 fallback)
    pub use_cpu_readback: bool,
    /// Desktop output index for capture/desktop-grab input selection
    pub output_index: u32,
}
impl EncoderConfig {
    /// Create encoder configuration with explicit parameters
    pub fn new(
        bitrate_mbps: u32,
        framerate: u32,
        resolution: (u32, u32),
        encoder_type: crate::config::EncoderType,
        keyframe_interval_secs: u32,
    ) -> Self {
        Self {
            bitrate_mbps,
            framerate,
            resolution,
            use_native_resolution: false,
            encoder_type,
            quality_preset: crate::config::QualityPreset::Balanced,
            rate_control: crate::config::RateControl::Vbr,
            quality_value: None,
            keyframe_interval_secs,
            use_cpu_readback: true,
            output_index: 0,
        }
    }
    /// Get the FFmpeg HEVC encoder name based on encoder type
    pub fn ffmpeg_codec_name(&self) -> &'static str {
        match self.encoder_type {
            crate::config::EncoderType::Nvenc => "hevc_nvenc",
            crate::config::EncoderType::Amf => "hevc_amf",
            crate::config::EncoderType::Qsv => "hevc_qsv",
            crate::config::EncoderType::Auto => "hevc_amf", // Default fallback for Auto
        }
    }
    /// Calculate keyframe interval in frames
    pub fn keyframe_interval_frames(&self) -> u32 {
        self.keyframe_interval_secs * self.framerate
    }

    pub fn supports_gpu_frame_transport(&self) -> bool {
        // AMF encoder accepts NV12 hardware frames via D3D11.
        // The capture layer now produces NV12 textures via Video Processor.
        matches!(self.encoder_type, crate::config::EncoderType::Amf)
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
    /// Health events emitted by encoder worker thread
    pub health_rx: Receiver<EncoderHealthEvent>,
    /// Effective encoder configuration after auto-selection/fallback decisions
    pub effective_config: EncoderConfig,
}
