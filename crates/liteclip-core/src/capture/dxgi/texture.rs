use std::mem::ManuallyDrop;
use std::ptr::null_mut;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use bytes::Bytes;
use tracing::{debug, info, warn};
use windows::Win32::Foundation::BOOL;
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11Texture2D, ID3D11VideoContext, ID3D11VideoDevice,
    ID3D11VideoProcessorEnumerator, ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView,
    D3D11_BIND_RENDER_TARGET, D3D11_RESOURCE_MISC_SHARED, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_NOMINAL_RANGE_0_255, D3D11_VIDEO_PROCESSOR_NOMINAL_RANGE_16_235,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_STREAM,
    D3D11_VPIV_DIMENSION_TEXTURE2D, D3D11_VPOV_DIMENSION_TEXTURE2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12};
use windows::Win32::Graphics::Dxgi::IDXGIResource;
use windows_core::Interface;

use crate::capture::{CapturedFrame, D3d11Frame, D3d11TexturePoolItem, GpuTextureFormat};

use super::capture::DxgiCaptureState;
use super::DxgiCapture;

impl DxgiCapture {
    pub(super) fn source_texture_for_frame(
        state: &DxgiCaptureState,
        captured_texture: &ID3D11Texture2D,
        _preferred_gpu_format: Option<GpuTextureFormat>,
    ) -> Result<(ID3D11Texture2D, (u32, u32))> {
        Ok((
            captured_texture.clone(),
            (state.frame_width, state.frame_height),
        ))
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
                output_view: Some(output_view.context("Pooled NV12 output view is null")?),
                shared_handle,
            })
        }
    }

    pub(super) fn create_bgra_pool_item(
        d3d_device: &ID3D11Device,
        width: u32,
        height: u32,
    ) -> Result<D3d11TexturePoolItem> {
        unsafe {
            let bgra_desc = windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: windows::Win32::Graphics::Direct3D11::D3D11_USAGE_DEFAULT,
                BindFlags: 0,
                CPUAccessFlags: 0,
                MiscFlags: D3D11_RESOURCE_MISC_SHARED.0 as u32,
            };
            let mut texture: Option<ID3D11Texture2D> = None;
            d3d_device
                .CreateTexture2D(&bgra_desc, None, Some(&mut texture))
                .ok()
                .context("Failed to create pooled BGRA shared texture")?;
            let texture = texture.context("Pooled BGRA texture is null")?;

            let dxgi_resource: IDXGIResource = texture
                .cast()
                .context("Failed to get IDXGIResource from pooled BGRA texture")?;
            let shared_handle = dxgi_resource
                .GetSharedHandle()
                .context("Failed to get shared handle for pooled BGRA texture")?;

            Ok(D3d11TexturePoolItem {
                texture,
                output_view: None,
                shared_handle,
            })
        }
    }

    pub(super) fn acquire_bgra_pool_item(
        state: &mut DxgiCaptureState,
    ) -> Result<D3d11TexturePoolItem> {
        let (width, height, maybe_item, at_capacity) = {
            let pool = state
                .bgra_pool
                .as_mut()
                .context("BGRA pool is not initialized")?;
            while let Ok(item) = pool.return_rx.try_recv() {
                pool.available.push(item);
            }
            let at_capacity = pool
                .total_created
                .load(std::sync::atomic::Ordering::Relaxed)
                >= pool.max_capacity;
            (pool.width, pool.height, pool.available.pop(), at_capacity)
        };

        if let Some(item) = maybe_item {
            return Ok(item);
        }

        if at_capacity {
            warn!("BGRA texture pool at capacity, waiting for texture to return");
            let pool = state
                .bgra_pool
                .as_mut()
                .context("BGRA pool is not initialized")?;
            match pool.return_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(item) => return Ok(item),
                Err(_) => bail!("BGRA texture pool exhausted, cannot acquire texture"),
            }
        }

        let pool = state
            .bgra_pool
            .as_mut()
            .context("BGRA pool is not initialized")?;
        pool.total_created
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        debug!(
            "Created new BGRA texture (total: {}/{})",
            pool.total_created
                .load(std::sync::atomic::Ordering::Relaxed),
            pool.max_capacity
        );
        Self::create_bgra_pool_item(&state.d3d_device, width, height)
    }

    pub(super) fn acquire_nv12_pool_item(
        state: &mut DxgiCaptureState,
    ) -> Result<D3d11TexturePoolItem> {
        let (width, height, maybe_item, at_capacity) = {
            let pool = state
                .nv12_pool
                .as_mut()
                .context("NV12 pool is not initialized")?;
            while let Ok(item) = pool.return_rx.try_recv() {
                pool.available.push(item);
            }
            let at_capacity = pool
                .total_created
                .load(std::sync::atomic::Ordering::Relaxed)
                >= pool.max_capacity;
            (pool.width, pool.height, pool.available.pop(), at_capacity)
        };

        if let Some(item) = maybe_item {
            return Ok(item);
        }

        if at_capacity {
            warn!("NV12 texture pool at capacity, waiting for texture to return");
            let pool = state
                .nv12_pool
                .as_mut()
                .context("NV12 pool is not initialized")?;
            match pool.return_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(item) => return Ok(item),
                Err(_) => bail!("NV12 texture pool exhausted, cannot acquire texture"),
            }
        }

        let pool = state
            .nv12_pool
            .as_mut()
            .context("NV12 pool is not initialized")?;
        pool.total_created
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        debug!(
            "Created new NV12 texture (total: {}/{})",
            pool.total_created
                .load(std::sync::atomic::Ordering::Relaxed),
            pool.max_capacity
        );

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
                video_context.VideoProcessorSetStreamColorSpace(
                    video_processor,
                    0,
                    &input_color_space,
                );
                video_context
                    .VideoProcessorSetOutputColorSpace(video_processor, &output_color_space);

                state.video_context = Some(video_context);
            }
            let video_context = state
                .video_context
                .as_ref()
                .context("Video context is None")?;
            let pooled_output_view = pooled_output
                .output_view
                .as_ref()
                .context("NV12 pooled output view is missing")?;

            let input_texture_ptr = bgra_texture.as_raw() as isize;
            let input_view = if let Some(view) = state.input_view_cache.get(&input_texture_ptr) {
                view.clone()
            } else {
                // Cap cache at 16 entries to prevent unbounded growth on repeated
                // resolution changes or pool expansions. Evict only one oldest entry
                // to preserve most warm cache hits under steady-state capture.
                const INPUT_VIEW_CACHE_LIMIT: usize = 16;
                if state.input_view_cache.len() >= INPUT_VIEW_CACHE_LIMIT {
                    if let Some(oldest_key) = state.input_view_cache_fifo.pop_front() {
                        let _ = state.input_view_cache.remove(&oldest_key);
                        debug!(
                            "input_view_cache evicted one entry (limit {} reached)",
                            INPUT_VIEW_CACHE_LIMIT
                        );
                    }
                }
                let view = Self::create_video_processor_input_view(state, bgra_texture)?;
                state
                    .input_view_cache
                    .insert(input_texture_ptr, view.clone());
                state.input_view_cache_fifo.push_back(input_texture_ptr);
                view
            };

            let stream_data_arr = [D3D11_VIDEO_PROCESSOR_STREAM {
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
            }];

            let res = video_context.VideoProcessorBlt(
                video_processor,
                pooled_output_view,
                0,
                &stream_data_arr,
            );

            let [stream_data] = stream_data_arr;

            // Un-leak the ManuallyDrop COM pointer wrapper
            let _ = ManuallyDrop::into_inner(stream_data.pInputSurface);

            res.ok().context("VideoProcessorBlt failed")?;

            // Always use GPU fence for non-blocking synchronization
            // Fallback to Flush only if fence is unavailable (should not happen on Win10+)
            if let Some(ref sync_fence) = state.nv12_sync_fence {
                state.nv12_fence_value += 1;
                let ctx4 = state
                    .d3d_context4
                    .as_ref()
                    .context("ID3D11DeviceContext4 unavailable for NV12 fence signal")?;
                ctx4.Signal(sync_fence, state.nv12_fence_value)
                    .context("Failed to signal NV12 sync fence")?;
            } else {
                // Log warning when falling back to Flush - this causes CPU stalls
                warn!("NV12 sync fence unavailable, falling back to Flush() - this may cause input latency");
                state.d3d_context.Flush();
            }

            Ok(pooled_output)
        }
    }

    pub(super) fn capture_gpu_frame(
        state: &mut DxgiCaptureState,
        captured_texture: &ID3D11Texture2D,
        timestamp: i64,
        gpu_texture_format: GpuTextureFormat,
    ) -> Result<CapturedFrame> {
        let (source_texture, resolution) =
            Self::source_texture_for_frame(state, captured_texture, Some(gpu_texture_format))?;
        let now = Instant::now();

        if gpu_texture_format == GpuTextureFormat::Bgra {
            let pooled_output = Self::acquire_bgra_pool_item(state)?;
            unsafe {
                let source_resource = source_texture
                    .cast()
                    .context("Failed to cast BGRA source texture to resource")?;
                let output_resource = pooled_output
                    .texture
                    .cast()
                    .context("Failed to cast BGRA pooled texture to resource")?;
                state
                    .d3d_context
                    .CopyResource(Some(&output_resource), Some(&source_resource));

                // Always use GPU fence for non-blocking synchronization
                if let Some(ref sync_fence) = state.bgra_sync_fence {
                    state.bgra_fence_value += 1;
                    let ctx4 = state
                        .d3d_context4
                        .as_ref()
                        .context("ID3D11DeviceContext4 unavailable for BGRA fence signal")?;
                    ctx4.Signal(sync_fence, state.bgra_fence_value)
                        .context("Failed to signal BGRA sync fence")?;
                } else {
                    // Log warning when falling back to Flush - this causes CPU stalls
                    warn!("BGRA sync fence unavailable, falling back to Flush() - this may cause input latency");
                    state.d3d_context.Flush();
                }
            }

            return Ok(CapturedFrame {
                bgra: Bytes::new(),
                d3d11: Some(Arc::new(D3d11Frame::from_pooled(
                    state.d3d_device.clone(),
                    GpuTextureFormat::Bgra,
                    pooled_output,
                    state.bgra_fence_value,
                    state.bgra_fence_shared_handle,
                    state
                        .bgra_pool
                        .as_ref()
                        .context("BGRA texture pool not initialized")?
                        .return_tx
                        .clone(),
                ))),
                timestamp,
                resolution,
            });
        }

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
                            nv12_item,
                            state.nv12_fence_value,
                            state.nv12_fence_shared_handle,
                            state
                                .nv12_pool
                                .as_ref()
                                .context("NV12 texture pool not initialized")?
                                .return_tx
                                .clone(),
                        ))),
                        timestamp,
                        resolution,
                    });
                }
                Err(e) => {
                    state.nv12_runtime_failures = state.nv12_runtime_failures.saturating_add(1);
                    let shift = state.nv12_runtime_failures.saturating_sub(1).min(4);
                    let retry_secs = 1u64 << shift;
                    let retry_delay = Duration::from_secs(retry_secs.min(16));
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
}
