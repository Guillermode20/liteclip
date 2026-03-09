//! DXGI Desktop Duplication Capture
//!
//! This module implements screen capture using Windows DXGI Desktop Duplication API.
//!
//! # Features
//!
//! - GPU-accelerated screen capture with minimal overhead
//! - GPU-side scaling and format conversion (NV12)
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
//! ```ignore
//! use liteclip_replay::capture::dxgi::DxgiCapture;
//! use liteclip_replay::capture::CaptureConfig;
//!
//! let mut capture = DxgiCapture::new();
//! let config = CaptureConfig::default();
//!
//! capture.start(config)?;
//!
//! // Receive captured frames
//! while let Ok(frame) = capture.frame_rx().recv() {
//!     // Process frame
//! }
//! ```

pub mod capture;
pub mod device;
pub mod dxgicapture_traits;
mod functions;
pub mod texture;
mod types;

#[allow(unused_imports)]
pub use capture::*;
#[allow(unused_imports)]
pub use device::*;
#[allow(unused_imports)]
pub use texture::*;
#[allow(unused_imports)]
pub use types::*;
