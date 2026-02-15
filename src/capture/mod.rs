//! Screen Capture Engine
//!
//! Acquires raw frames from the desktop/game using DXGI Desktop Duplication.

use crate::d3d::D3D11Texture;
use anyhow::Result;
use bytes::Bytes;
use crossbeam::channel::Receiver;
use std::time::Duration;

pub mod dxgi;

/// Configuration for screen capture
#[derive(Debug, Clone)]
pub struct CaptureConfig {
    pub target_fps: u32,
    pub output_index: u32,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            target_fps: 60,
            output_index: 0,
        }
    }
}

impl From<&crate::config::Config> for CaptureConfig {
    fn from(config: &crate::config::Config) -> Self {
        Self {
            target_fps: config.video.framerate,
            output_index: config.advanced.gpu_index,
        }
    }
}

/// Captured frame data
pub struct CapturedFrame {
    /// GPU-resident texture (no CPU copy)
    pub texture: D3D11Texture,
    /// CPU-readable BGRA frame bytes (packed, width*height*4).
    /// Uses `Bytes` for reference-counted sharing – cloning is O(1).
    pub bgra: Bytes,
    /// QPC timestamp for sync
    pub timestamp: i64,
    /// Frame resolution (width, height)
    pub resolution: (u32, u32),
}

// Manual Clone implementation because D3D11Texture may not implement Clone
impl Clone for CapturedFrame {
    fn clone(&self) -> Self {
        Self {
            texture: self.texture.clone(),
            bgra: self.bgra.clone(),
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

/// Calculate frame duration from target FPS
pub fn frame_duration(fps: u32) -> Duration {
    Duration::from_nanos(1_000_000_000 / fps as u64)
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
