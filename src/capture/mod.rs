//! Screen and Audio Capture Engine
//!
//! This module provides capture functionality for screen content via DXGI
//! Desktop Duplication and audio via WASAPI.
//!
//! # Architecture
//!
//! ## Video Capture (DXGI)
//!
//! DXGI Desktop Duplication provides GPU-accelerated screen capture with
//! minimal overhead. The capture runs on a dedicated thread and produces
//! frames at the configured framerate.
//!
//! Key features:
//! - Zero-copy GPU texture capture for hardware encoders
//! - GPU-side scaling via Direct3D11 VideoProcessor
//! - NV12 conversion for AMF encoder compatibility
//! - CPU readback fallback for software encoding
//!
//! ## Audio Capture (WASAPI)
//!
//! WASAPI captures system audio and microphone input with low latency.
//! Multiple audio streams are mixed together before encoding.
//!
//! # Key Types
//!
//! - [`CaptureConfig`] - Configuration for screen capture
//! - [`CapturedFrame`] - A captured video frame with CPU and/or GPU data
//! - [`CaptureBackend`] - Trait for capture implementations
//! - [`DxgiCapture`] - DXGI Desktop Duplication implementation
//!
//! # Example
//!
//! ```ignore
//! use liteclip_replay::capture::{DxgiCapture, CaptureConfig, CaptureBackend};
//!
//! let mut capture = DxgiCapture::new();
//! let config = CaptureConfig {
//!     target_fps: 60,
//!     output_index: 0,
//!     perform_cpu_readback: true,
//!     target_resolution: Some((1920, 1080)),
//! };
//!
//! capture.start(config)?;
//!
//! // Receive frames
//! while let Ok(frame) = capture.frame_rx().recv() {
//!     // Process frame
//! }
//! ```

use anyhow::Result;
use bytes::Bytes;
use crossbeam::channel::{Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;
#[cfg(windows)]
use windows::Win32::Foundation::HANDLE;
#[cfg(windows)]
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11Texture2D, ID3D11VideoProcessorOutputView,
};

pub mod audio;
pub mod backpressure;
pub mod dxgi;
pub mod frame;

pub use dxgi::DxgiCapture;

/// Configuration for screen capture.
///
/// Controls capture behavior including framerate, output selection, and
/// resolution handling.
///
/// # Example
///
/// ```
/// use liteclip_replay::capture::CaptureConfig;
///
/// let config = CaptureConfig {
///     target_fps: 60,
///     output_index: 0,
///     perform_cpu_readback: true,
///     target_resolution: Some((1920, 1080)),
/// };
/// ```
#[derive(Debug, Clone)]
pub struct CaptureConfig {
    /// Target frames per second for capture.
    pub target_fps: u32,
    /// Display output index to capture (0 = primary monitor).
    pub output_index: u32,
    /// Whether to perform CPU readback of GPU textures.
    /// Required for software encoders; optional for hardware encoders.
    pub perform_cpu_readback: bool,
    /// Target resolution for captured frames.
    /// If None, uses native desktop resolution.
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

/// GPU texture format for D3D11 frames.
///
/// Determines the pixel format of GPU textures used for capture and encoding.
/// Different encoders may require different formats for optimal performance.
#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuTextureFormat {
    /// BGRA format (B8G8R8A8_UNORM).
    ///
    /// This is the default capture format from DXGI Desktop Duplication.
    /// Compatible with NVENC and most software encoders.
    Bgra,
    /// NV12 format (YUV 4:2:0).
    ///
    /// Required for AMD AMF encoder. Converted from BGRA using
    /// Direct3D11 VideoProcessor for GPU-accelerated conversion.
    Nv12,
}

/// Pool item for recycled D3D11 textures.
///
/// Textures are pooled and recycled to avoid allocation overhead during
/// active capture. When a frame is processed, the texture is returned to
/// the pool for reuse.
#[cfg(windows)]
pub(crate) struct D3d11TexturePoolItem {
    /// The D3D11 texture containing frame data.
    pub texture: ID3D11Texture2D,
    /// Video processor output view for GPU-side processing.
    pub output_view: ID3D11VideoProcessorOutputView,
    /// DXGI shared resource handle for cross-device sharing.
    ///
    /// Allows the encoder to open this texture on its own D3D11 device.
    /// GPU ordering is provided by a shared ID3D11Fence — the capture
    /// signals it after VideoProcessorBlt and the encoder GPU-waits on
    /// it before CopySubresourceRegion.
    pub shared_handle: HANDLE,
}

#[cfg(windows)]
struct D3d11TextureRecycle {
    return_tx: Sender<D3d11TexturePoolItem>,
    item: Option<D3d11TexturePoolItem>,
}

#[cfg(windows)]
impl Drop for D3d11TextureRecycle {
    fn drop(&mut self) {
        if let Some(item) = self.item.take() {
            let _ = self.return_tx.send(item);
        }
    }
}

#[cfg(windows)]
pub struct D3d11Frame {
    pub texture: ID3D11Texture2D,
    pub device: ID3D11Device,
    /// Texture format - indicates whether this is BGRA or NV12
    pub format: GpuTextureFormat,
    /// DXGI shared resource handle for the encoder to open this texture on its own D3D11 device.
    pub shared_handle: HANDLE,
    /// The fence value that was signaled after the VideoProcessorBlt that wrote this frame.
    /// The encoder submits a GPU-side Wait for this value before CopySubresourceRegion,
    /// guaranteeing cross-device ordering without any CPU stall.
    pub fence_value: u64,
    /// NT kernel handle for the shared ID3D11Fence. The encoder opens the fence once via
    /// OpenSharedFence and caches it. None if the fence could not be created.
    pub fence_shared_handle: Option<HANDLE>,
    #[allow(dead_code)]
    recycle: Option<D3d11TextureRecycle>,
}

// HANDLE wraps a raw *mut c_void (a Windows kernel handle index) which is safe to send
// and share between threads — kernel objects are reference-counted at the kernel level.
// COM interfaces (ID3D11Texture2D, ID3D11Device) already implement Send+Sync in the
// windows crate.
#[cfg(windows)]
unsafe impl Send for D3d11Frame {}
#[cfg(windows)]
unsafe impl Sync for D3d11Frame {}

#[cfg(windows)]
impl D3d11Frame {
    pub(crate) fn from_pooled(
        device: ID3D11Device,
        format: GpuTextureFormat,
        return_tx: Sender<D3d11TexturePoolItem>,
        pool_item: D3d11TexturePoolItem,
        fence_value: u64,
        fence_shared_handle: Option<HANDLE>,
    ) -> Self {
        let texture = pool_item.texture.clone();
        let shared_handle = pool_item.shared_handle;
        Self {
            texture,
            device,
            format,
            shared_handle,
            fence_value,
            fence_shared_handle,
            recycle: Some(D3d11TextureRecycle {
                return_tx,
                item: Some(pool_item),
            }),
        }
    }
}

/// Captured frame data containing both CPU and GPU representations.
///
/// A captured frame may contain:
/// - CPU-accessible BGRA pixel data (for software encoding)
/// - GPU texture reference (for hardware encoding with zero-copy)
///
/// The frame includes timing information for A/V synchronization.
///
/// # Thread Safety
///
/// The `bgra` data uses `Bytes` for cheap reference-counted cloning.
/// GPU texture access is managed via `Arc` for shared ownership.
pub struct CapturedFrame {
    /// CPU-readable BGRA frame bytes (packed, width*height*4).
    ///
    /// Uses `Bytes` for reference-counted sharing – cloning is O(1).
    pub bgra: Bytes,
    /// Optional GPU-backed frame payload for zero-copy encoder paths.
    #[cfg(windows)]
    pub d3d11: Option<Arc<D3d11Frame>>,
    /// QPC (QueryPerformanceCounter) timestamp for A/V synchronization.
    pub timestamp: i64,
    /// Frame resolution as (width, height) in pixels.
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

/// Capture backend trait for video capture implementations.
///
/// Implementations must be `Send` to allow capture on a dedicated thread.
///
/// # Implementors
///
/// - [`DxgiCapture`] - DXGI Desktop Duplication for Windows screen capture
pub trait CaptureBackend: Send + 'static {
    /// Start capturing frames with the given configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if capture initialization fails (e.g., no display).
    fn start(&mut self, config: CaptureConfig) -> Result<()>;

    /// Stop capturing frames and release resources.
    fn stop(&mut self);

    /// Get the receiver for captured frames.
    ///
    /// Frames are sent to this channel as they are captured.
    fn frame_rx(&self) -> Receiver<CapturedFrame>;
}

/// Calculate frame duration from target FPS.
///
/// Returns the duration between frames at the given framerate.
/// Clamps FPS to a minimum of 1 to prevent division by zero.
///
/// # Arguments
///
/// * `fps` - Target frames per second.
///
/// # Returns
///
/// Duration between frames.
///
/// # Example
///
/// ```
/// use liteclip_replay::capture::frame_duration;
///
/// let duration = frame_duration(60);
/// assert_eq!(duration.as_nanos(), 16_666_666);
/// ```
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
