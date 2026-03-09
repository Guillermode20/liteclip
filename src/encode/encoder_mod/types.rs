//! Encoder types
//!
//! This module provides core types for the encoding subsystem, including
//! packet types, configuration, and encoder handles.

use anyhow::Result;
use bytes::Bytes;
use crossbeam::channel::{Receiver, Sender};

/// Encoder health events for error propagation.
#[derive(Debug, Clone)]
pub enum EncoderHealthEvent {
    /// Fatal error that requires pipeline restart.
    Fatal(String),
}

/// Encoded packet data from video or audio encoder.
///
/// Uses `Bytes` for reference-counted data, making clones cheap (just a ref count bump).
/// This is critical for the snapshot operation which needs to quickly clone the entire
/// buffer when saving clips.
///
/// # Thread Safety
///
/// The `data` field is cheaply cloneable via `Bytes`.
#[derive(Clone)]
pub struct EncodedPacket {
    /// Reference-counted byte buffer (cheap clone).
    pub data: Bytes,
    /// Presentation timestamp (QPC-based, 10MHz units).
    pub pts: i64,
    /// Decode timestamp.
    pub dts: i64,
    /// True if this is a keyframe (IDR frame for video).
    pub is_keyframe: bool,
    /// Stream type (video or audio).
    pub stream: StreamType,
    /// Optional frame resolution for raw video payloads.
    pub resolution: Option<(u32, u32)>,
}

impl EncodedPacket {
    /// Creates a new encoded packet.
    ///
    /// # Arguments
    ///
    /// * `data` - The encoded data.
    /// * `pts` - Presentation timestamp.
    /// * `dts` - Decode timestamp.
    /// * `is_keyframe` - Whether this is a keyframe.
    /// * `stream` - The stream type.
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

    /// Creates a video keyframe packet.
    ///
    /// # Arguments
    ///
    /// * `data` - The encoded keyframe data.
    /// * `pts` - Presentation timestamp.
    pub fn video_keyframe(data: impl Into<Bytes>, pts: i64) -> Self {
        Self::new(data, pts, pts, true, StreamType::Video)
    }

    /// Creates a video delta frame packet.
    ///
    /// # Arguments
    ///
    /// * `data` - The encoded delta frame data.
    /// * `pts` - Presentation timestamp.
    pub fn video_delta(data: impl Into<Bytes>, pts: i64) -> Self {
        Self::new(data, pts, pts, false, StreamType::Video)
    }
}

/// Stream type for multiplexed output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamType {
    /// Video stream (HEVC/H.264).
    Video,
    /// System audio (game/desktop audio).
    SystemAudio,
    /// Microphone input.
    Microphone,
}

/// Hardware encoder detection result.
///
/// Indicates which hardware encoder (if any) is available on the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardwareEncoder {
    /// NVIDIA NVENC available.
    Nvenc,
    /// AMD AMF available.
    Amf,
    /// Intel QSV available.
    Qsv,
    /// No hardware encoder available.
    None,
}

/// Encoder configuration for video encoding.
///
/// Controls encoding parameters including bitrate, framerate, resolution,
/// encoder selection (NVENC/AMF/QSV/software), and quality settings.
#[derive(Debug, Clone)]
pub struct EncoderConfig {
    /// Target bitrate in Mbps.
    pub bitrate_mbps: u32,
    /// Target framerate.
    pub framerate: u32,
    /// Output resolution as (width, height).
    pub resolution: (u32, u32),
    /// Whether to use native capture resolution (ignores resolution field).
    pub use_native_resolution: bool,
    /// Encoder type selection (NVENC, AMF, QSV, software).
    pub encoder_type: crate::config::EncoderType,
    /// Quality preset preference (speed vs quality).
    pub quality_preset: crate::config::QualityPreset,
    /// Rate control mode preference.
    pub rate_control: crate::config::RateControl,
    /// Optional quality scalar (e.g., CQ/CRF-like).
    pub quality_value: Option<u8>,
    /// Keyframe interval in seconds.
    pub keyframe_interval_secs: u32,
    /// Force CPU readback path (Phase 1 fallback).
    pub use_cpu_readback: bool,
    /// Desktop output index for capture/desktop-grab input selection.
    pub output_index: u32,
}

impl EncoderConfig {
    /// Creates encoder configuration with explicit parameters.
    ///
    /// # Arguments
    ///
    /// * `bitrate_mbps` - Target bitrate in Mbps.
    /// * `framerate` - Target framerate.
    /// * `resolution` - Output resolution as (width, height).
    /// * `encoder_type` - The encoder to use.
    /// * `keyframe_interval_secs` - Keyframe interval in seconds.
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
        // The capture produces NV12 textures tagged with D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX
        // so the encoder can open them on its own isolated D3D11 device without sharing the
        // capture thread's command context or multithread lock.
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
