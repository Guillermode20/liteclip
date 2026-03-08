//! Screen and Audio Capture Engine
//!
//! Acquires raw frames from the desktop/game using DXGI Desktop Duplication
//! and captures audio using WASAPI.

use crate::encode::EncodedPacket;
use anyhow::Result;
use bytes::Bytes;
use crossbeam::channel::Receiver;
use std::sync::Arc;
use std::time::Duration;
#[cfg(windows)]
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11Texture2D};

pub mod audio;
pub mod backpressure;
pub mod dxgi;

/// Configuration for screen capture
#[derive(Debug, Clone)]
pub struct CaptureConfig {
    pub target_fps: u32,
    pub output_index: u32,
    pub perform_cpu_readback: bool,
    pub target_resolution: Option<(u32, u32)>,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            target_fps: 60,
            output_index: 0,
            perform_cpu_readback: true,
            target_resolution: None,
        }
    }
}

impl From<&crate::config::Config> for CaptureConfig {
    fn from(config: &crate::config::Config) -> Self {
        let target_resolution = config.video.target_resolution();
        Self {
            target_fps: config.video.framerate,
            output_index: config.advanced.gpu_index,
            perform_cpu_readback: config.advanced.use_cpu_readback,
            target_resolution,
        }
    }
}

/// GPU texture format for D3D11 frames
#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuTextureFormat {
    /// BGRA format (B8G8R8A8_UNORM) - default capture format
    Bgra,
    /// NV12 format - required for hardware encoders like AMF
    Nv12,
}

#[cfg(windows)]
pub struct D3d11Frame {
    pub texture: ID3D11Texture2D,
    pub device: ID3D11Device,
    /// Texture format - indicates whether this is BGRA or NV12
    pub format: GpuTextureFormat,
}

/// Captured frame data
pub struct CapturedFrame {
    /// CPU-readable BGRA frame bytes (packed, width*height*4).
    /// Uses `Bytes` for reference-counted sharing – cloning is O(1).
    pub bgra: Bytes,
    /// Optional GPU-backed frame payload for zero-copy encoder paths.
    #[cfg(windows)]
    pub d3d11: Option<Arc<D3d11Frame>>,
    /// QPC timestamp for sync
    pub timestamp: i64,
    /// Frame resolution (width, height)
    pub resolution: (u32, u32),
}

impl Clone for CapturedFrame {
    fn clone(&self) -> Self {
        Self {
            bgra: self.bgra.clone(),
            #[cfg(windows)]
            d3d11: self.d3d11.clone(),
            timestamp: self.timestamp,
            resolution: self.resolution,
        }
    }
}

/// Capture backend trait
pub trait CaptureBackend: Send + 'static {
    /// Start capturing frames
    fn start(&mut self, config: CaptureConfig) -> Result<()>;

    /// Stop capturing frames
    fn stop(&mut self);

    /// Get receiver for captured frames
    fn frame_rx(&self) -> Receiver<CapturedFrame>;
}

/// Audio capture backend trait
pub trait AudioCaptureBackend: Send + 'static {
    /// Start capturing audio
    fn start(&mut self, config: &crate::config::AudioConfig) -> Result<()>;

    /// Stop capturing audio
    fn stop(&mut self);

    /// Get receiver for captured audio packets
    fn packet_rx(&self) -> Receiver<EncodedPacket>;

    /// Check if audio capture is running
    fn is_running(&self) -> bool;
}

/// Calculate frame duration from target FPS
///
/// Clamps fps to a minimum of 1 to prevent division by zero.
pub fn frame_duration(fps: u32) -> Duration {
    Duration::from_nanos(1_000_000_000 / fps.max(1) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frame_duration() {
        let duration = frame_duration(30);
        assert_eq!(duration.as_millis(), 33);
    }

    #[test]
    fn test_frame_duration_60fps() {
        let duration = frame_duration(60);
        assert_eq!(duration.as_nanos(), 16_666_666);
    }
}
