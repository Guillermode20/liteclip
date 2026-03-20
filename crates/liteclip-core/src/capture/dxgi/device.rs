use anyhow::{Context, Result};
use std::sync::atomic::Ordering;
use tracing::debug;
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11VideoDevice, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_RATIONAL,
};
use windows::Win32::Graphics::Dxgi::CreateDXGIFactory1;
use windows_core::Interface;

use super::{capture::DxgiCaptureState, DxgiCapture};

impl DxgiCapture {
    /// Validate that NV12 conversion is supported by the GPU
    /// This creates a temporary D3D11 device and checks Video Processor support
    pub fn validate_nv12_capability() -> Result<bool> {
        Self::validate_nv12_capability_for_output(0)
    }

    pub fn validate_nv12_capability_for_output(output_index: u32) -> Result<bool> {
        unsafe {
            // Create a temporary DXGI factory and D3D11 device
            let factory: windows::Win32::Graphics::Dxgi::IDXGIFactory1 = CreateDXGIFactory1()
                .context("Failed to create DXGI factory for NV12 validation")?;

            // Try to find the requested output on any adapter.
            let mut adapter_index = 0u32;
            let mut selected_adapter = None;
            loop {
                let adapter = match factory.EnumAdapters1(adapter_index) {
                    Ok(adapter) => adapter,
                    Err(_) => break,
                };

                match adapter.EnumOutputs(output_index) {
                    Ok(_) => {
                        selected_adapter = Some(adapter);
                        break;
                    }
                    Err(_) => {
                        adapter_index += 1;
                    }
                }
            }

            let adapter = selected_adapter.context(format!(
                "No adapter with output {} found for NV12 validation",
                output_index
            ))?;

            let mut d3d_device: Option<ID3D11Device> = None;
            let feature_levels = [
                windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_1,
                windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0,
            ];
            let adapter_for_device: windows::Win32::Graphics::Dxgi::IDXGIAdapter =
                adapter.cast().context("Failed to cast adapter")?;

            windows::Win32::Graphics::Direct3D11::D3D11CreateDevice(
                Some(&adapter_for_device),
                windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN,
                windows::Win32::Foundation::HMODULE::default(),
                windows::Win32::Graphics::Direct3D11::D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&feature_levels),
                windows::Win32::Graphics::Direct3D11::D3D11_SDK_VERSION,
                Some(&mut d3d_device),
                None,
                None,
            )
            .ok()
            .context("Failed to create D3D11 device for NV12 validation")?;

            let d3d_device = d3d_device.context("D3D11 device is null")?;

            // Check if the device supports video processing
            let video_device: ID3D11VideoDevice = match d3d_device.cast() {
                Ok(vd) => vd,
                Err(_) => {
                    debug!("D3D11 device does not support video processing interface");
                    return Ok(false);
                }
            };

            // Create video processor enumerator to check format support
            let content_desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat:
                    windows::Win32::Graphics::Direct3D11::D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputFrameRate: DXGI_RATIONAL {
                    Numerator: 60,
                    Denominator: 1,
                },
                InputWidth: 1920,
                InputHeight: 1080,
                OutputFrameRate: DXGI_RATIONAL {
                    Numerator: 60,
                    Denominator: 1,
                },
                OutputWidth: 1920,
                OutputHeight: 1080,
                Usage: windows::Win32::Graphics::Direct3D11::D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
            };

            let enumerator = match video_device.CreateVideoProcessorEnumerator(&content_desc) {
                Ok(e) => e,
                Err(_) => {
                    debug!("Failed to create video processor enumerator");
                    return Ok(false);
                }
            };

            // Check BGRA input format support
            let input_caps = match enumerator.CheckVideoProcessorFormat(DXGI_FORMAT_B8G8R8A8_UNORM)
            {
                Ok(caps) => caps,
                Err(_) => {
                    debug!("Failed to check BGRA format support");
                    return Ok(false);
                }
            };
            let input_supported = (input_caps
                & windows::Win32::Graphics::Direct3D11::D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT.0
                    as u32)
                != 0;
            if !input_supported {
                debug!("Video processor does not support BGRA input");
                return Ok(false);
            }

            // Check NV12 output format support
            let output_caps = match enumerator.CheckVideoProcessorFormat(DXGI_FORMAT_NV12) {
                Ok(caps) => caps,
                Err(_) => {
                    debug!("Failed to check NV12 format support");
                    return Ok(false);
                }
            };
            let output_supported = (output_caps
                & windows::Win32::Graphics::Direct3D11::D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT
                    .0 as u32)
                != 0;
            if !output_supported {
                debug!("Video processor does not support NV12 output");
                return Ok(false);
            }

            // Try to create a video processor to ensure full support
            let video_processor = match video_device.CreateVideoProcessor(&enumerator, 0) {
                Ok(vp) => vp,
                Err(_) => {
                    debug!("Failed to create video processor");
                    return Ok(false);
                }
            };

            // All checks passed
            drop(video_processor);
            debug!(
                "NV12 conversion capability validated on output {}: BGRA->NV12 supported",
                output_index
            );
            Ok(true)
        }
    }

    /// Check if NV12 GPU conversion is available for zero-copy encoding
    pub fn is_nv12_conversion_capable(&self) -> bool {
        self.nv12_conversion_capable
    }

    pub fn try_recv_fatal(&self) -> Option<String> {
        self.fatal_rx.try_recv().ok()
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    pub fn is_capture_thread_finished(&self) -> bool {
        self.capture_thread
            .as_ref()
            .is_some_and(|thread| thread.is_finished())
    }

    /// Initialize D3D11 device and DXGI duplication
    pub(super) fn init_capture(output_index: u32) -> Result<DxgiCaptureState> {
        DxgiCaptureState::init_capture_with_scaling(output_index, None)
    }

    /// Initialize D3D11 device and DXGI duplication with a configured output size (encoder scales).
    pub(super) fn init_capture_with_target(
        output_index: u32,
        target_resolution: (u32, u32),
    ) -> Result<DxgiCaptureState> {
        DxgiCaptureState::init_capture_with_scaling(output_index, Some(target_resolution))
    }
}
