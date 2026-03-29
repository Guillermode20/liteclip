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
//! - D3D11 Video Processor BGRA→NV12 when available for hardware encoders
//! - Encoder-side resize when configured output size differs from the desktop
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
//! ```no_run
//! use liteclip_core::capture::CaptureConfig;
//!
//! let mut config = CaptureConfig::default();
//! config.target_fps = 60;
//! config.target_resolution = Some((1920, 1080));
//! ```

use anyhow::Result;
use crossbeam::channel::Receiver;

#[cfg(windows)]
pub use crate::media::{CapturedFrame, D3d11Frame, D3d11TexturePoolItem, GpuTextureFormat};

#[cfg(not(windows))]
pub use crate::media::CapturedFrame;

pub mod audio;
pub mod backpressure;
pub mod dxgi;
pub mod error;

pub use dxgi::{DxgiCapture, DxgiCaptureFactory};
pub use error::{CaptureError, CaptureResult};

/// Configuration for screen capture.
///
/// Controls capture behavior including framerate, output selection, and
/// resolution handling.
///
/// # Example
///
/// ```ignore
/// use liteclip_core::capture::CaptureConfig;
///
/// let mut config = CaptureConfig::default();
/// config.target_fps = 60;
/// config.target_resolution = Some((1920, 1080));
/// ```
#[derive(Debug, Clone)]
pub struct CaptureConfig {
    /// Target frames per second for capture.
    pub target_fps: u32,
    /// Display output index to capture (0 = primary monitor).
    pub output_index: u32,
    /// Preferred GPU texture format for hardware encoder transport.
    #[cfg(windows)]
    pub gpu_texture_format: GpuTextureFormat,
    /// Target resolution for captured frames.
    /// If None, uses native desktop resolution.
    pub target_resolution: Option<(u32, u32)>,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            target_fps: 60,
            output_index: 0,
            #[cfg(windows)]
            gpu_texture_format: GpuTextureFormat::Nv12,
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
            #[cfg(windows)]
            gpu_texture_format: GpuTextureFormat::Nv12,
            target_resolution,
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

    /// Check if a fatal error has occurred.
    ///
    /// Returns the error message if a fatal error was detected.
    fn try_recv_fatal(&self) -> Option<String> {
        None
    }

    /// Check if capture is currently running.
    fn is_running(&self) -> bool;

    /// Check if the capture thread has finished.
    fn is_capture_thread_finished(&self) -> bool;
}

/// Factory trait for creating capture backend instances.
///
/// This abstraction allows dependency injection for testing and
/// supports alternative capture backends (e.g., Vulkan, Linux).
///
/// # Thread Safety
///
/// Implementations must be `Send + Sync` to allow sharing across threads.
pub trait CaptureFactory: Send + Sync + 'static {
    /// Create a new capture backend instance.
    ///
    /// # Errors
    ///
    /// Returns an error if the capture backend cannot be created.
    fn create(&self) -> Result<Box<dyn CaptureBackend>>;

    /// Check if NV12 conversion is supported for the given output.
    ///
    /// This is used to determine GPU-side format conversion capability.
    fn refresh_nv12_capability(&self, output_index: u32) -> bool;
}
