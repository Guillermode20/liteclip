use std::mem::ManuallyDrop;
use std::ptr::null_mut;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use bytes::Bytes;
use tracing::{debug, info, warn};
use windows::Win32::Foundation::BOOL;
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Buffer, ID3D11Device, ID3D11DeviceContext4, ID3D11RenderTargetView,
    ID3D11SamplerState, ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11VideoContext,
    ID3D11VideoDevice, ID3D11VideoProcessorEnumerator,
    ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView,
    D3D11_BIND_RENDER_TARGET, D3D11_RESOURCE_MISC_SHARED,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_NOMINAL_RANGE_0_255,
    D3D11_VIDEO_PROCESSOR_NOMINAL_RANGE_16_235, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VPIV_DIMENSION_TEXTURE2D, D3D11_VPOV_DIMENSION_TEXTURE2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12};
use windows::Win32::Graphics::Dxgi::IDXGIResource;
use windows_core::Interface;

use crate::capture::{CapturedFrame, D3d11Frame, D3d11TexturePoolItem, GpuTextureFormat};

use super::capture::{DxgiCaptureState, Vertex};
use super::DxgiCapture;

impl DxgiCapture {
    /// Create or resize staging texture for frame readback
    /// When GPU scaling is enabled, staging texture is at target resolution
    pub(super) fn ensure_staging_texture(state: &mut DxgiCaptureState) -> Result<()> {
        unsafe {
            if state.staging_texture.is_some() {
                return Ok(());
            }
            let (width, height) = if state.scale_texture.is_some() {
                (state.target_width, state.target_height)
            } else {
                (state.frame_width, state.frame_height)
            };
            let desc = windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: windows::Win32::Graphics::Direct3D11::D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: windows::Win32::Graphics::Direct3D11::D3D11_CPU_ACCESS_READ.0
                    as u32,
                MiscFlags: 0,
            };
            let mut texture = None;
            state
                .d3d_device
                .CreateTexture2D(&desc, None, Some(&mut texture))
                .ok()
                .context("Failed to create staging texture")?;
            state.staging_texture = texture;
            debug!("Created staging texture: {}x{}", width, height);
            Ok(())
        }
    }

    pub(super) fn source_texture_for_frame(
        state: &DxgiCaptureState,
        captured_texture: &ID3D11Texture2D,
    ) -> Result<(ID3D11Texture2D, (u32, u32))> {
        let has_target_scaling = state.scale_texture.is_some();
        let needs_separate_gpu_scale = has_target_scaling && !state.nv12_conversion_available;
        let source_texture = if needs_separate_gpu_scale {
            state
                .scale_texture
                .as_ref()
                .context("Scale texture is None")?
                .clone()
        } else {
            captured_texture.clone()
        };
        let resolution = if has_target_scaling {
            (state.target_width, state.target_height)
        } else {
            (state.frame_width, state.frame_height)
        };
        Ok((source_texture, resolution))
    }

    pub(super) fn create_video_processor_input_view(
        state: &DxgiCaptureState,
        texture: &ID3D11Texture2D,
    ) -> Result<ID3D11VideoProcessorInputView> {
        unsafe {
            let video_device = state
                .video_device
                .as_ref()
                .context("Video device not initialized")?;
            let enumerator = state
                .video_processor_enumerator
                .as_ref()
                .context("Video processor enumerator is None")?;
            let input_view_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                FourCC: 0,
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                Anonymous: std::mem::zeroed(),
            };
            let mut input_view: Option<ID3D11VideoProcessorInputView> = None;
            video_device
                .CreateVideoProcessorInputView(
                    texture,
                    enumerator,
                    &input_view_desc,
                    Some(&mut input_view),
                )
                .ok()
                .context("Failed to create video processor input view")?;
            input_view.context("Input view is null")
        }
    }

    pub(super) fn create_nv12_pool_item(
        d3d_device: &ID3D11Device,
        video_device: &ID3D11VideoDevice,
        enumerator: &ID3D11VideoProcessorEnumerator,
        width: u32,
        height: u32,
    ) -> Result<D3d11TexturePoolItem> {
        unsafe {
            let nv12_desc = windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_NV12,
                SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: windows::Win32::Graphics::Direct3D11::D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: D3D11_RESOURCE_MISC_SHARED.0 as u32,
            };
            let mut texture: Option<ID3D11Texture2D> = None;
            d3d_device
                .CreateTexture2D(&nv12_desc, None, Some(&mut texture))
                .ok()
                .context("Failed to create pooled NV12 shared texture")?;
            let texture = texture.context("Pooled NV12 texture is null")?;

            let dxgi_resource: IDXGIResource = texture
                .cast()
                .context("Failed to get IDXGIResource from pooled NV12 texture")?;
            let shared_handle = dxgi_resource
                .GetSharedHandle()
                .context("Failed to get shared handle for pooled NV12 texture")?;

            let output_view_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                Anonymous: std::mem::zeroed(),
            };
            let mut output_view: Option<ID3D11VideoProcessorOutputView> = None;
            video_device
                .CreateVideoProcessorOutputView(
                    &texture,
                    enumerator,
                    &output_view_desc,
                    Some(&mut output_view),
                )
                .ok()
                .context("Failed to create pooled NV12 output view")?;

            Ok(D3d11TexturePoolItem {
                texture,
                output_view: output_view.context("Pooled NV12 output view is null")?,
                shared_handle,
            })
        }
    }

    pub(super) fn acquire_nv12_pool_item(
        state: &mut DxgiCaptureState,
    ) -> Result<D3d11TexturePoolItem> {
        let (width, height, maybe_item) = {
            let pool = state
                .nv12_pool
                .as_mut()
                .context("NV12 pool is not initialized")?;
            while let Ok(item) = pool.return_rx.try_recv() {
                pool.available.push(item);
            }
            (pool.width, pool.height, pool.available.pop())
        };

        if let Some(item) = maybe_item {
            return Ok(item);
        }

        let video_device = state
            .video_device
            .as_ref()
            .context("Video device not initialized")?;
        let enumerator = state
            .video_processor_enumerator
            .as_ref()
            .context("Video processor enumerator is None")?;
        Self::create_nv12_pool_item(&state.d3d_device, video_device, enumerator, width, height)
    }

    pub(super) fn convert_bgra_to_nv12(
        state: &mut DxgiCaptureState,
        bgra_texture: &ID3D11Texture2D,
    ) -> Result<D3d11TexturePoolItem> {
        unsafe {
            let pooled_output = Self::acquire_nv12_pool_item(state)?;
            let Some(ref video_processor) = state.video_processor else {
                bail!("Video processor not initialized");
            };

            if state.video_context.is_none() {
                let video_context: ID3D11VideoContext = state
                    .d3d_context
                    .cast()
                    .context("Failed to get video context from device context")?;

                let input_color_space = DxgiCaptureState::video_processor_color_space(
                    true,
                    true,
                    D3D11_VIDEO_PROCESSOR_NOMINAL_RANGE_0_255.0 as u32,
                );
                let output_color_space = DxgiCaptureState::video_processor_color_space(
                    true,
                    true,
                    D3D11_VIDEO_PROCESSOR_NOMINAL_RANGE_16_235.0 as u32,
                );
                video_context
                    .VideoProcessorSetStreamColorSpace(video_processor, 0, &input_color_space);
                video_context.VideoProcessorSetOutputColorSpace(video_processor, &output_color_space);

                state.video_context = Some(video_context);
            }
            let video_context = state
                .video_context
                .as_ref()
                .context("Video context is None")?;
            let using_cached_scale_input = state
                .scale_texture
                .as_ref()
                .is_some_and(|scale_texture| scale_texture.as_raw() == bgra_texture.as_raw());
            let input_view = if using_cached_scale_input {
                if state.scale_input_view.is_none() {
                    state.scale_input_view = Some(Self::create_video_processor_input_view(
                        state,
                        bgra_texture,
                    )?);
                }
                state
                    .scale_input_view
                    .as_ref()
                    .context("Scale input view is null")?
                    .clone()
            } else {
                Self::create_video_processor_input_view(state, bgra_texture)?
            };

            let stream_data = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: BOOL(1),
                OutputIndex: 0,
                InputFrameOrField: 0,
                PastFrames: 0,
                FutureFrames: 0,
                ppPastSurfaces: null_mut(),
                pInputSurface: ManuallyDrop::new(Some(input_view)),
                ppFutureSurfaces: null_mut(),
                ppPastSurfacesRight: null_mut(),
                pInputSurfaceRight: ManuallyDrop::new(None),
                ppFutureSurfacesRight: null_mut(),
            };

            video_context
                .VideoProcessorBlt(
                    video_processor,
                    &pooled_output.output_view,
                    0,
                    &[stream_data],
                )
                .ok()
                .context("VideoProcessorBlt failed")?;

            if let Some(ref sync_fence) = state.nv12_sync_fence {
                state.nv12_fence_value += 1;
                let ctx4: ID3D11DeviceContext4 = state
                    .d3d_context
                    .cast()
                    .context("Failed to get ID3D11DeviceContext4 for fence signal")?;
                ctx4.Signal(sync_fence, state.nv12_fence_value)
                    .context("Failed to signal NV12 sync fence")?;
            } else {
                state.d3d_context.Flush();
            }

            Ok(pooled_output)
        }
    }

    pub(super) fn capture_gpu_frame(
        state: &mut DxgiCaptureState,
        captured_texture: &ID3D11Texture2D,
        timestamp: i64,
    ) -> Result<CapturedFrame> {
        let (source_texture, resolution) = Self::source_texture_for_frame(state, captured_texture)?;
        let now = Instant::now();

        let should_try_nv12 = state.nv12_conversion_available
            && match state.nv12_retry_after {
                Some(retry_after) => now >= retry_after,
                None => true,
            };

        if should_try_nv12 {
            match Self::convert_bgra_to_nv12(state, &source_texture) {
                Ok(nv12_item) => {
                    if state.nv12_runtime_failures > 0 {
                        info!(
                            "NV12 GPU conversion recovered after {} runtime failure(s)",
                            state.nv12_runtime_failures
                        );
                    }
                    state.nv12_runtime_failures = 0;
                    state.nv12_retry_after = None;

                    return Ok(CapturedFrame {
                        bgra: Bytes::new(),
                        d3d11: Some(Arc::new(D3d11Frame::from_pooled(
                            state.d3d_device.clone(),
                            GpuTextureFormat::Nv12,
                            state
                                .nv12_pool
                                .as_ref()
                                .context("NV12 pool is not initialized")?
                                .return_tx
                                .clone(),
                            nv12_item,
                            state.nv12_fence_value,
                            state.nv12_fence_shared_handle,
                        ))),
                        timestamp,
                        resolution,
                    });
                }
                Err(e) => {
                    state.nv12_runtime_failures = state.nv12_runtime_failures.saturating_add(1);
                    let retry_delay = Duration::from_secs(2);
                    state.nv12_retry_after = Some(now + retry_delay);
                    warn!(
                        "NV12 conversion failed (attempt {}), dropping frame and backing off for {:.1}s: {}",
                        state.nv12_runtime_failures,
                        retry_delay.as_secs_f32(),
                        e
                    );
                    bail!("NV12 GPU conversion failed: {}", e);
                }
            }
        }

        if !state.nv12_conversion_available && !state.nv12_unavailable_logged {
            warn!("NV12 conversion resources are unavailable on the selected capture device");
            state.nv12_unavailable_logged = true;
        }

        bail!(
            "NV12 GPU conversion unavailable (conversion_available={}, in_retry_backoff={})",
            state.nv12_conversion_available,
            state.nv12_retry_after.is_some()
        );
    }

    /// Perform GPU-side downscaling using pixel shader
    /// This renders the captured texture to a smaller render target
    pub(super) fn perform_gpu_scale(
        state: &mut DxgiCaptureState,
        captured_texture: &ID3D11Texture2D,
    ) -> Result<()> {
        unsafe {
            let Some(ref _scale_texture) = state.scale_texture else {
                return Ok(()); // No scaling needed
            };

            let Some(ref vertex_shader) = state.vertex_shader else {
                bail!("GPU scaling resources not initialized");
            };
            let Some(ref pixel_shader) = state.pixel_shader else {
                bail!("GPU scaling resources not initialized");
            };
            let Some(ref input_layout) = state.input_layout else {
                bail!("GPU scaling resources not initialized");
            };
            let Some(ref sampler) = state.sampler else {
                bail!("GPU scaling resources not initialized");
            };
            let Some(ref vertex_buffer) = state.vertex_buffer else {
                bail!("GPU scaling resources not initialized");
            };
            let Some(ref rtv) = state.rtv else {
                bail!("GPU scaling resources not initialized");
            };

            // Create shader resource view for the captured texture
            let mut srv: Option<ID3D11ShaderResourceView> = None;
            state
                .d3d_device
                .CreateShaderResourceView(captured_texture, None, Some(&mut srv))
                .ok()
                .context("Failed to create SRV for captured texture")?;
            let srv = srv.context("SRV is null")?;

            // Clear render target (optional, but good practice)
            let clear_color = [0.0f32; 4];
            state.d3d_context.ClearRenderTargetView(rtv, &clear_color);

            // Set up the graphics pipeline for rendering
            state.d3d_context.IASetInputLayout(Some(input_layout));

            let stride = std::mem::size_of::<Vertex>() as u32;
            let offset = 0u32;
            let vb: Option<ID3D11Buffer> = Some(vertex_buffer.clone());
            state
                .d3d_context
                .IASetVertexBuffers(0, 1, Some(&vb), Some(&stride), Some(&offset));
            state.d3d_context.IASetPrimitiveTopology(
                windows::Win32::Graphics::Direct3D::D3D11_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP,
            );

            state.d3d_context.VSSetShader(Some(vertex_shader), None);
            state.d3d_context.PSSetShader(Some(pixel_shader), None);
            state
                .d3d_context
                .PSSetSamplers(0, Some(&[Some::<ID3D11SamplerState>(sampler.clone())]));
            state
                .d3d_context
                .PSSetShaderResources(0, Some(&[Some(srv)]));

            // Set render target
            state
                .d3d_context
                .OMSetRenderTargets(Some(&[Some::<ID3D11RenderTargetView>(rtv.clone())]), None);

            // Set viewport for target resolution
            let viewport = windows::Win32::Graphics::Direct3D11::D3D11_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: state.target_width as f32,
                Height: state.target_height as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            };
            state.d3d_context.RSSetViewports(Some(&[viewport]));

            // Draw fullscreen quad (4 vertices as triangle strip)
            state.d3d_context.Draw(4, 0);

            // Unbind shader resources to avoid conflicts
            state.d3d_context.PSSetShaderResources(0, Some(&[None]));
            state.d3d_context.OMSetRenderTargets(None, None);

            Ok(())
        }
    }

}
