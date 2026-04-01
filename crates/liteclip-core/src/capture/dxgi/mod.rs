//! DXGI Desktop Duplication Capture
//!
//! This module implements screen capture using Windows DXGI Desktop Duplication API.
//!
//! # Features
//!
//! - GPU-accelerated screen capture with minimal overhead
//! - BGRA→NV12 via D3D11 Video Processor when available
//! - Cross-device texture sharing for zero-copy encoding
//! - Multi-monitor support
//!
//! # Architecture
//!
//! The capture runs on a dedicated thread:
//!
//! 1. Initialize DXGI factory and output duplication
//! 2. Create D3D11 device context for GPU operations
//! 3. Capture frames via `AcquireNextFrame`
//! 4. Optionally convert to NV12 for AMF encoder
//! 5. Share textures with encoder via DXGI shared handles
//!
//! # Key Types
//!
//! - [`DxgiCapture`] - Main capture implementation
//! - [`CaptureConfig`] - Capture configuration
//! - [`CapturedFrame`] - Frame data with CPU/GPU representations
//!
//! # Example
//!
//! ```no_run
//! use liteclip_core::capture::dxgi::DxgiCapture;
//! use liteclip_core::capture::CaptureConfig;
//!
//! let capture = DxgiCapture::new().unwrap();
//! // capture.start(config).unwrap();
//! ```

pub mod capture;
pub mod device;
pub mod dxgicapture_traits;
pub mod texture;

pub use capture::*;

use crate::capture::{CaptureBackend, CaptureFactory};
use anyhow::Result;

/// Factory for creating DXGI capture instances.
///
/// This is the default capture factory for Windows, using DXGI Desktop Duplication.
pub struct DxgiCaptureFactory;

impl CaptureFactory for DxgiCaptureFactory {
    fn create(&self) -> Result<Box<dyn CaptureBackend>> {
        let capture = DxgiCapture::new()?;
        Ok(Box::new(capture))
    }

    fn refresh_nv12_capability(&self, output_index: u32) -> bool {
        DxgiCapture::validate_nv12_capability_for_output(output_index).unwrap_or(false)
    }
}

/// Detects the resolution of the specified display output.
///
/// Returns the native resolution (width, height) of the display at the given index.
/// Returns `None` if the display cannot be accessed or doesn't exist.
///
/// # Arguments
/// * `output_index` - Display output index (0 = primary monitor)
///
/// # Example
/// ```ignore
/// if let Some((width, height)) = detect_display_resolution(0) {
///     println!("Primary display: {}x{}", width, height);
/// }
/// ```
#[cfg(windows)]
pub fn detect_display_resolution(output_index: u32) -> Option<(u32, u32)> {
    use tracing::warn;
    use windows::Win32::Graphics::Dxgi::CreateDXGIFactory1;

    let factory: windows::Win32::Graphics::Dxgi::IDXGIFactory1 =
        unsafe { CreateDXGIFactory1() }.ok()?;

    let mut adapter_index = 0;
    let mut output_count = 0u32;

    while let Ok(adapter) = unsafe { factory.EnumAdapters1(adapter_index) } {
        let mut output_idx = 0;

        while let Ok(output) = unsafe { adapter.EnumOutputs(output_idx) } {
            if output_count == output_index {
                // Found the target output
                let desc = unsafe { output.GetDesc() }.ok()?;
                let width =
                    (desc.DesktopCoordinates.right - desc.DesktopCoordinates.left).unsigned_abs();
                let height =
                    (desc.DesktopCoordinates.bottom - desc.DesktopCoordinates.top).unsigned_abs();

                if width > 0 && height > 0 {
                    return Some((width, height));
                } else {
                    warn!(
                        "Detected display {} has invalid dimensions: {}x{}",
                        output_index, width, height
                    );
                    return None;
                }
            }
            output_count += 1;
            output_idx += 1;
        }

        adapter_index += 1;
    }

    warn!("Display output {} not found", output_index);
    None
}

#[cfg(not(windows))]
pub fn detect_display_resolution(_output_index: u32) -> Option<(u32, u32)> {
    None
}
