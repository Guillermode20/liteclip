//! DXGI capture with GPU-side scaling support

use crate::capture::{
    backpressure::BackpressureState, CaptureConfig, CapturedFrame, D3d11Frame,
    D3d11TexturePoolItem, GpuTextureFormat,
};
use anyhow::{bail, Context, Result};
use bytes::{Bytes, BytesMut};
use crossbeam::channel::{bounded, unbounded, Receiver, Sender};
use std::mem::ManuallyDrop;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};
use windows::Win32::Foundation::{BOOL, HANDLE};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Buffer, ID3D11Device, ID3D11Device5, ID3D11DeviceContext, ID3D11DeviceContext4,
    ID3D11Fence, ID3D11InputLayout, ID3D11Multithread, ID3D11PixelShader, ID3D11RenderTargetView,
    ID3D11Resource, ID3D11SamplerState, ID3D11ShaderResourceView, ID3D11Texture2D,
    ID3D11VertexShader, ID3D11VideoContext, ID3D11VideoDevice, ID3D11VideoProcessor,
    ID3D11VideoProcessorEnumerator, ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView,
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_VERTEX_BUFFER, D3D11_FENCE_FLAG_SHARED,
    D3D11_INPUT_ELEMENT_DESC, D3D11_INPUT_PER_VERTEX_DATA, D3D11_MAPPED_SUBRESOURCE,
    D3D11_MAP_READ, D3D11_RESOURCE_MISC_SHARED, D3D11_USAGE_IMMUTABLE,
    D3D11_VIDEO_PROCESSOR_COLOR_SPACE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_NOMINAL_RANGE_0_255,
    D3D11_VIDEO_PROCESSOR_NOMINAL_RANGE_16_235, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VPIV_DIMENSION_TEXTURE2D, D3D11_VPOV_DIMENSION_TEXTURE2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_FORMAT_R32G32_FLOAT, DXGI_RATIONAL,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIOutput1, IDXGIResource, DXGI_ERROR_ACCESS_DENIED,
    DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_INVALID_CALL, DXGI_ERROR_NON_COMPOSITED_UI,
    DXGI_ERROR_NOT_CURRENTLY_AVAILABLE, DXGI_ERROR_UNSUPPORTED, DXGI_ERROR_WAIT_TIMEOUT,
    DXGI_OUTDUPL_FRAME_INFO, DXGI_OUTPUT_DESC,
};
use windows::Win32::System::Performance::QueryPerformanceCounter;
use windows_core::Interface;

/// Simple vertex for fullscreen quad: position (x, y) and texcoord (u, v)
#[repr(C)]
#[derive(Clone, Copy)]
struct Vertex {
    x: f32,
    y: f32,
    u: f32,
    v: f32,
}

struct Nv12TexturePool {
    available: Vec<D3d11TexturePoolItem>,
    return_tx: Sender<D3d11TexturePoolItem>,
    return_rx: Receiver<D3d11TexturePoolItem>,
    width: u32,
    height: u32,
}

impl Nv12TexturePool {
    fn new(width: u32, height: u32) -> Self {
        let (return_tx, return_rx) = unbounded();
        Self {
            available: Vec::new(),
            return_tx,
            return_rx,
            width,
            height,
        }
    }
}

/// DXGI capture state with GPU-side scaling support
struct DxgiCaptureState {
    d3d_device: ID3D11Device,
    d3d_context: ID3D11DeviceContext,
    duplication: windows::Win32::Graphics::Dxgi::IDXGIOutputDuplication,
    #[allow(dead_code)]
    output_desc: DXGI_OUTPUT_DESC,
    staging_texture: Option<ID3D11Texture2D>,
    frame_width: u32,
    frame_height: u32,
    target_width: u32,
    target_height: u32,
    vertex_shader: Option<ID3D11VertexShader>,
    pixel_shader: Option<ID3D11PixelShader>,
    input_layout: Option<ID3D11InputLayout>,
    sampler: Option<ID3D11SamplerState>,
    vertex_buffer: Option<ID3D11Buffer>,
    rtv: Option<ID3D11RenderTargetView>,
    scale_texture: Option<ID3D11Texture2D>,
    native_buffer: BytesMut,
    nv12_pool: Option<Nv12TexturePool>,
    video_device: Option<ID3D11VideoDevice>,
    video_context: Option<ID3D11VideoContext>,
    video_processor: Option<ID3D11VideoProcessor>,
    video_processor_enumerator: Option<ID3D11VideoProcessorEnumerator>,
    scale_input_view: Option<ID3D11VideoProcessorInputView>,
    nv12_conversion_available: bool,
    nv12_runtime_failures: u32,
    nv12_retry_after: Option<Instant>,
    nv12_unavailable_logged: bool,
    /// Shared ID3D11Fence used for GPU-side cross-device synchronization.
    /// After VideoProcessorBlt the capture GPU queue signals this fence; the encoder GPU queue
    /// waits on it before CopySubresourceRegion. No CPU stall — ordering is fully on the GPU.
    nv12_sync_fence: Option<ID3D11Fence>,
    /// NT kernel handle for `nv12_sync_fence` (D3D11_FENCE_FLAG_SHARED).
    /// Passed through D3d11Frame so the encoder can call OpenSharedFence once on its own device.
    nv12_fence_shared_handle: Option<HANDLE>,
    /// Monotonically increasing value. Incremented and signaled before each frame hand-off.
    nv12_fence_value: u64,
}

impl DxgiCaptureState {
    fn video_processor_color_space(
        rgb_full_range: bool,
        ycbcr_bt709: bool,
        nominal_range: u32,
    ) -> D3D11_VIDEO_PROCESSOR_COLOR_SPACE {
        let mut bitfield = 0u32;
        if !rgb_full_range {
            bitfield |= 1 << 1;
        }
        if ycbcr_bt709 {
            bitfield |= 1 << 2;
        }
        bitfield |= (nominal_range & 0b11) << 4;
        D3D11_VIDEO_PROCESSOR_COLOR_SPACE {
            _bitfield: bitfield,
        }
    }

    pub fn get_qpc_timestamp() -> i64 {
        unsafe {
            let mut qpc = 0i64;
            QueryPerformanceCounter(&mut qpc).expect("QueryPerformanceCounter should never fail");
            qpc
        }
    }

    /// Initialize DXGI capture state with optional GPU-side scaling
    fn init_capture_with_scaling(
        output_index: u32,
        target_resolution: Option<(u32, u32)>,
    ) -> Result<Self> {
        unsafe {
            let factory: windows::Win32::Graphics::Dxgi::IDXGIFactory1 =
                CreateDXGIFactory1().context("Failed to create DXGI factory")?;
            let mut adapter_index = 0u32;
            let mut selected_adapter = None;
            loop {
                let adapter = match factory.EnumAdapters1(adapter_index) {
                    Ok(adapter) => adapter,
                    Err(_) => break,
                };

                match adapter.EnumOutputs(output_index) {
                    Ok(output) => {
                        selected_adapter = Some((adapter, output));
                        break;
                    }
                    Err(_) => {
                        adapter_index += 1;
                    }
                }
            }

            let (adapter, output) =
                selected_adapter.context("Failed to find adapter with requested output index")?;
            let output: IDXGIOutput1 = output
                .cast()
                .context("Failed to cast output to IDXGIOutput1")?;

            let output_desc = output
                .GetDesc()
                .context("Failed to get output description")?;

            let mut d3d_device: Option<ID3D11Device> = None;
            let mut d3d_context: Option<ID3D11DeviceContext> = None;
            let feature_levels = [
                windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_1,
                windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0,
                windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_10_1,
                windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_10_0,
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
                Some(&mut d3d_context),
            )
            .ok()
            .context("Failed to create D3D11 device")?;
            let d3d_device = d3d_device.context("D3D11 device is null")?;
            let d3d_context = d3d_context.context("D3D11 context is null")?;
            Self::enable_multithread_protection(&d3d_device);

            let duplication = output
                .DuplicateOutput(&d3d_device)
                .map_err(|e| {
                    let code = e.code().0;
                    let msg = match code {
                        c if c == DXGI_ERROR_ACCESS_DENIED.0 => {
                            "Access denied - screen capture requires admin privileges or the Desktop Window Manager must be running"
                        }
                        c if c == DXGI_ERROR_ACCESS_LOST.0 => {
                            "Access lost - desktop composition may be disabled"
                        }
                        c if c == DXGI_ERROR_INVALID_CALL.0 => "Invalid call",
                        c if c == DXGI_ERROR_NON_COMPOSITED_UI.0 => {
                            "Non-composited desktop - DWM must be enabled"
                        }
                        c if c == DXGI_ERROR_NOT_CURRENTLY_AVAILABLE.0 => {
                            "Not currently available - another application may be capturing"
                        }
                        c if c == DXGI_ERROR_UNSUPPORTED.0 => "Unsupported",
                        c if c == DXGI_ERROR_WAIT_TIMEOUT.0 => "Timeout (unexpected)",
                        _ => "Unknown error",
                    };
                    anyhow::anyhow!(
                        "Failed to duplicate output: {} (0x{:08X})", msg, code as u32
                    )
                })?;

            let frame_width =
                (output_desc.DesktopCoordinates.right - output_desc.DesktopCoordinates.left) as u32;
            let frame_height =
                (output_desc.DesktopCoordinates.bottom - output_desc.DesktopCoordinates.top) as u32;

            let (target_width, target_height) =
                target_resolution.unwrap_or((frame_width, frame_height));
            let needs_scaling = target_width != frame_width || target_height != frame_height;

            // Initialize GPU scaling resources if needed
            let (
                vertex_shader,
                pixel_shader,
                input_layout,
                sampler,
                vertex_buffer,
                rtv,
                scale_texture,
            ) = if needs_scaling {
                // First, try to initialize shader resources
                match Self::init_gpu_scaling_resources(&d3d_device, frame_width, frame_height) {
                    Ok((vs, ps, il, smp, vb)) => {
                        // Shaders compiled successfully, now create the scale texture
                        let desc = windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC {
                            Width: target_width,
                            Height: target_height,
                            MipLevels: 1,
                            ArraySize: 1,
                            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                                Count: 1,
                                Quality: 0,
                            },
                            Usage: windows::Win32::Graphics::Direct3D11::D3D11_USAGE_DEFAULT,
                            BindFlags:
                                windows::Win32::Graphics::Direct3D11::D3D11_BIND_RENDER_TARGET.0
                                    as u32,
                            CPUAccessFlags: 0,
                            MiscFlags: 0,
                        };
                        let mut texture = None;
                        if d3d_device
                            .CreateTexture2D(&desc, None, Some(&mut texture))
                            .is_ok()
                        {
                            // Create render target view for the scale texture
                            let mut rtv: Option<ID3D11RenderTargetView> = None;
                            if d3d_device
                                .CreateRenderTargetView(
                                    texture.as_ref().unwrap(),
                                    None,
                                    Some(&mut rtv),
                                )
                                .is_ok()
                            {
                                info!(
                                    "GPU scaling enabled: {}x{} -> {}x{}",
                                    frame_width, frame_height, target_width, target_height
                                );
                                (vs, ps, il, smp, vb, rtv, texture)
                            } else {
                                warn!("Failed to create RTV for scale texture, using CPU scaling");
                                (None, None, None, None, None, None, None)
                            }
                        } else {
                            warn!("Failed to create scale texture, using CPU scaling");
                            (None, None, None, None, None, None, None)
                        }
                    }
                    Err(e) => {
                        warn!("GPU scaling not available: {}, using CPU scaling", e);
                        (None, None, None, None, None, None, None)
                    }
                }
            } else {
                (None, None, None, None, None, None, None)
            };

            // Initialize NV12 conversion resources for hardware encoding
            let (
                nv12_pool,
                video_device,
                video_context,
                video_processor,
                video_processor_enumerator,
                scale_input_view,
                nv12_conversion_available,
            ) = Self::init_nv12_conversion_resources(&d3d_device, target_width, target_height)
                .unwrap_or_else(|e| {
                    warn!(
                        "NV12 conversion unavailable during capture init: {}; GPU-preferred capture will fall back to CPU readback",
                        e
                    );
                    (None, None, None, None, None, None, false)
                });

            if nv12_conversion_available {
                info!("NV12 conversion enabled for GPU zero-copy encoding");
            }

            // Create a shared ID3D11Fence for GPU-side cross-device synchronization.
            // After VideoProcessorBlt the capture GPU queue signals the fence; the encoder GPU
            // queue waits on it before CopySubresourceRegion. This replaces the CPU spin loop
            // (D3D11_QUERY_EVENT) which held the DXGI frame during the stall and caused the
            // capture rate to drop below 60 fps whenever AMD's GPU took >16 ms for the BLT.
            let (nv12_sync_fence, nv12_fence_shared_handle) = if nv12_conversion_available {
                match d3d_device.cast::<ID3D11Device5>() {
                    Ok(device5) => {
                        let mut fence_opt: Option<ID3D11Fence> = None;
                        match device5.CreateFence(0, D3D11_FENCE_FLAG_SHARED, &mut fence_opt) {
                            Ok(()) => match fence_opt {
                                Some(fence) => {
                                    // 0x10000000 = GENERIC_ALL — full access for cross-device use
                                    match fence.CreateSharedHandle(
                                        None,
                                        0x10000000u32,
                                        windows_core::PCWSTR::null(),
                                    ) {
                                        Ok(handle) => {
                                            info!("NV12 sync fence created (GPU-side cross-device ordering)");
                                            (Some(fence), Some(handle))
                                        }
                                        Err(e) => {
                                            warn!("Failed to create shared fence handle, NV12 sync will fall back to Flush-only: {}", e);
                                            (None, None)
                                        }
                                    }
                                }
                                None => {
                                    warn!("CreateFence returned null, NV12 sync will fall back to Flush-only");
                                    (None, None)
                                }
                            },
                            Err(e) => {
                                warn!("Failed to create NV12 sync fence, NV12 sync will fall back to Flush-only: {}", e);
                                (None, None)
                            }
                        }
                    }
                    Err(e) => {
                        warn!(
                            "ID3D11Device5 unavailable, NV12 sync will fall back to Flush-only: {}",
                            e
                        );
                        (None, None)
                    }
                }
            } else {
                (None, None)
            };

            // Buffer size is based on target resolution (smaller if GPU scaling enabled)
            let buffer_size = if scale_texture.is_some() {
                (target_width * target_height * 4) as usize
            } else {
                // CPU scaling - read at native resolution, scale in encoder
                (frame_width * frame_height * 4) as usize
            };
            let mut native_buffer = BytesMut::with_capacity(buffer_size);
            native_buffer.resize(buffer_size, 0);

            let gpu_scaling_enabled = scale_texture.is_some();
            if gpu_scaling_enabled {
                info!(
                    "DXGI capture initialized with GPU scaling: {}x{} -> {}x{}",
                    frame_width, frame_height, target_width, target_height
                );
            } else if needs_scaling {
                info!(
                    "DXGI capture initialized (CPU scaling): {}x{} -> {}x{}",
                    frame_width, frame_height, target_width, target_height
                );
            } else {
                info!("DXGI capture initialized: {}x{}", frame_width, frame_height);
            }

            Ok(DxgiCaptureState {
                d3d_device,
                d3d_context,
                duplication,
                output_desc,
                staging_texture: None,
                frame_width,
                frame_height,
                target_width,
                target_height,
                vertex_shader,
                pixel_shader,
                input_layout,
                sampler,
                vertex_buffer,
                rtv,
                scale_texture,
                native_buffer,
                nv12_pool,
                video_device,
                video_context,
                video_processor,
                video_processor_enumerator,
                scale_input_view,
                nv12_conversion_available,
                nv12_runtime_failures: 0,
                nv12_retry_after: None,
                nv12_unavailable_logged: false,
                nv12_sync_fence,
                nv12_fence_shared_handle,
                nv12_fence_value: 0,
            })
        }
    }

    fn enable_multithread_protection(d3d_device: &ID3D11Device) {
        unsafe {
            match d3d_device.cast::<ID3D11Multithread>() {
                Ok(multithread) => {
                    let _ = multithread.SetMultithreadProtected(BOOL(1));
                }
                Err(error) => {
                    warn!("Failed to enable D3D11 multithread protection: {}", error);
                }
            }
        }
    }

    /// Initialize NV12 conversion resources using D3D11 Video Processor
    fn init_nv12_conversion_resources(
        d3d_device: &ID3D11Device,
        width: u32,
        height: u32,
    ) -> Result<(
        Option<Nv12TexturePool>,
        Option<ID3D11VideoDevice>,
        Option<ID3D11VideoContext>,
        Option<ID3D11VideoProcessor>,
        Option<ID3D11VideoProcessorEnumerator>,
        Option<ID3D11VideoProcessorInputView>,
        bool,
    )> {
        unsafe {
            // Try to get video device interface
            let video_device: ID3D11VideoDevice = d3d_device
                .cast()
                .context("D3D11 device does not support video processing")?;

            // Create video processor enumerator
            let content_desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat:
                    windows::Win32::Graphics::Direct3D11::D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputFrameRate: DXGI_RATIONAL {
                    Numerator: 60,
                    Denominator: 1,
                },
                InputWidth: width,
                InputHeight: height,
                OutputFrameRate: DXGI_RATIONAL {
                    Numerator: 60,
                    Denominator: 1,
                },
                OutputWidth: width,
                OutputHeight: height,
                Usage: windows::Win32::Graphics::Direct3D11::D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
            };
            let video_processor_enumerator = video_device
                .CreateVideoProcessorEnumerator(&content_desc)
                .context("Failed to create video processor enumerator")?;

            // Check if the video processor supports BGRA input and NV12 output
            let input_format = DXGI_FORMAT_B8G8R8A8_UNORM;
            let output_format = DXGI_FORMAT_NV12;

            // Check input format support (BGRA)
            let input_caps = video_processor_enumerator
                .CheckVideoProcessorFormat(input_format)
                .context("Failed to check input format support")?;
            let input_supported = (input_caps
                & windows::Win32::Graphics::Direct3D11::D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT.0
                    as u32)
                != 0;
            if !input_supported {
                bail!("Video processor does not support BGRA input format");
            }

            // Check output format support (NV12)
            let output_caps = video_processor_enumerator
                .CheckVideoProcessorFormat(output_format)
                .context("Failed to check output format support")?;
            let output_supported = (output_caps
                & windows::Win32::Graphics::Direct3D11::D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT
                    .0 as u32)
                != 0;
            if !output_supported {
                bail!("Video processor does not support NV12 output format");
            }

            // Create video processor
            let video_processor = video_device
                .CreateVideoProcessor(&video_processor_enumerator, 0)
                .context("Failed to create video processor")?;

            let mut nv12_pool = Nv12TexturePool::new(width, height);
            for _ in 0..4 {
                nv12_pool.available.push(Self::create_nv12_pool_item(
                    d3d_device,
                    &video_device,
                    &video_processor_enumerator,
                    width,
                    height,
                )?);
            }

            info!(
                "NV12 video processor initialized successfully for {}x{}",
                width, height
            );

            Ok((
                Some(nv12_pool),
                Some(video_device),
                None, // video_context will be obtained from d3d_context when needed
                Some(video_processor),
                Some(video_processor_enumerator),
                None,
                true,
            ))
        }
    }

    fn source_texture_for_frame(
        state: &DxgiCaptureState,
        captured_texture: &ID3D11Texture2D,
    ) -> Result<(ID3D11Texture2D, (u32, u32))> {
        let needs_gpu_scale = state.scale_texture.is_some();
        let source_texture = if needs_gpu_scale {
            state
                .scale_texture
                .as_ref()
                .context("Scale texture is None")?
                .clone()
        } else {
            captured_texture.clone()
        };
        let resolution = if needs_gpu_scale {
            (state.target_width, state.target_height)
        } else {
            (state.frame_width, state.frame_height)
        };
        Ok((source_texture, resolution))
    }

    fn create_video_processor_input_view(
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

    fn ensure_nv12_cached_views(state: &mut DxgiCaptureState) -> Result<()> {
        if state.scale_input_view.is_none() {
            if let Some(scale_texture) = state.scale_texture.as_ref() {
                state.scale_input_view = Some(Self::create_video_processor_input_view(
                    state,
                    scale_texture,
                )?);
            }
        }

        Ok(())
    }

    fn create_nv12_pool_item(
        d3d_device: &ID3D11Device,
        video_device: &ID3D11VideoDevice,
        enumerator: &ID3D11VideoProcessorEnumerator,
        width: u32,
        height: u32,
    ) -> Result<D3d11TexturePoolItem> {
        unsafe {
            // D3D11_RESOURCE_MISC_SHARED enables cross-device sharing via GetSharedHandle /
            // OpenSharedResource. GPU ordering is ensured by a D3D11_QUERY_EVENT stall on the
            // capture side before the frame is handed off; no keyed mutex is needed.
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

            // Get the DXGI shared handle so the encoder can open this texture on its own device.
            let dxgi_resource: IDXGIResource = texture
                .cast()
                .context("Failed to get IDXGIResource from pooled NV12 texture")?;
            let shared_handle: HANDLE = dxgi_resource
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

    fn acquire_nv12_pool_item(state: &mut DxgiCaptureState) -> Result<D3d11TexturePoolItem> {
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

    /// Convert BGRA texture to NV12 using Video Processor.
    ///
    /// Acquires the NV12 pool texture's keyed mutex (key=0 = capture's write turn) before the
    /// VideoProcessorBlt and releases it with key=1 afterward to signal the encoder it may read.
    fn convert_bgra_to_nv12(
        state: &mut DxgiCaptureState,
        bgra_texture: &ID3D11Texture2D,
    ) -> Result<D3d11TexturePoolItem> {
        unsafe {
            Self::ensure_nv12_cached_views(state)?;
            let pooled_output = Self::acquire_nv12_pool_item(state)?;
            let Some(ref video_processor) = state.video_processor else {
                bail!("Video processor not initialized");
            };

            // Get video context from D3D context (lazy initialization)
            if state.video_context.is_none() {
                let video_context: ID3D11VideoContext = state
                    .d3d_context
                    .cast()
                    .context("Failed to get video context from device context")?;
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
                state
                    .scale_input_view
                    .as_ref()
                    .context("Scale input view is null")?
                    .clone()
            } else {
                Self::create_video_processor_input_view(state, bgra_texture)?
            };

            let input_color_space = Self::video_processor_color_space(
                true,
                true,
                D3D11_VIDEO_PROCESSOR_NOMINAL_RANGE_0_255.0 as u32,
            );
            let output_color_space = Self::video_processor_color_space(
                true,
                true,
                D3D11_VIDEO_PROCESSOR_NOMINAL_RANGE_16_235.0 as u32,
            );
            video_context.VideoProcessorSetStreamColorSpace(video_processor, 0, &input_color_space);
            video_context.VideoProcessorSetOutputColorSpace(video_processor, &output_color_space);

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

            // Enqueue a GPU-side Signal into the capture device's command queue. The encoder
            // device's command queue will Wait on this fence value before CopySubresourceRegion,
            // guaranteeing that VideoProcessorBlt has completed before the encoder reads the
            // NV12 texture. This is entirely GPU-side — no CPU stall, no DXGI frame hold-up.
            if let Some(ref sync_fence) = state.nv12_sync_fence {
                state.nv12_fence_value += 1;
                let ctx4: ID3D11DeviceContext4 = state
                    .d3d_context
                    .cast()
                    .context("Failed to get ID3D11DeviceContext4 for fence signal")?;
                ctx4.Signal(sync_fence, state.nv12_fence_value)
                    .context("Failed to signal NV12 sync fence")?;
            }
            // Flush submits all pending GPU commands (VideoProcessorBlt + Signal) to the
            // hardware queue. This ensures the Signal is in-flight before the frame is handed
            // off to the encoder, so the encoder's Wait sees a valid fence value.
            state.d3d_context.Flush();

            Ok(pooled_output)
        }
    }

    fn capture_gpu_frame(
        state: &mut DxgiCaptureState,
        captured_texture: &ID3D11Texture2D,
        timestamp: i64,
    ) -> Result<CapturedFrame> {
        let (source_texture, resolution) = Self::source_texture_for_frame(state, captured_texture)?;
        let now = Instant::now();

        // Prefer GPU NV12 when available. If conversion fails at runtime, back off briefly
        // and keep using CPU readback until the next retry window.
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

    /// Initialize GPU scaling resources (shaders, vertex buffer)
    fn init_gpu_scaling_resources(
        d3d_device: &ID3D11Device,
        _src_width: u32,
        _src_height: u32,
    ) -> Result<(
        Option<ID3D11VertexShader>,
        Option<ID3D11PixelShader>,
        Option<ID3D11InputLayout>,
        Option<ID3D11SamplerState>,
        Option<ID3D11Buffer>,
    )> {
        unsafe {
            // Embed compiled shader bytecode at compile time so release builds do not depend on
            // Cargo build directories being present at runtime.
            let vs_bytecode = include_bytes!(concat!(env!("OUT_DIR"), "/vs_simple.cso"));
            let ps_bytecode = include_bytes!(concat!(env!("OUT_DIR"), "/ps_simple.cso"));

            // If shader compilation failed, disable GPU scaling
            if vs_bytecode.is_empty() || ps_bytecode.is_empty() {
                warn!("GPU scaling shaders not available, using CPU scaling");
                return Ok((None, None, None, None, None));
            }

            // Create vertex shader
            let mut vertex_shader: Option<ID3D11VertexShader> = None;
            d3d_device
                .CreateVertexShader(vs_bytecode, None, Some(&mut vertex_shader))
                .ok()
                .context("Failed to create vertex shader")?;

            // Create pixel shader
            let mut pixel_shader: Option<ID3D11PixelShader> = None;
            d3d_device
                .CreatePixelShader(ps_bytecode, None, Some(&mut pixel_shader))
                .ok()
                .context("Failed to create pixel shader")?;

            // Create input layout
            let input_element_descs = [
                D3D11_INPUT_ELEMENT_DESC {
                    SemanticName: windows::core::PCSTR(b"POSITION\0".as_ptr()),
                    SemanticIndex: 0,
                    Format: DXGI_FORMAT_R32G32_FLOAT,
                    InputSlot: 0,
                    AlignedByteOffset: 0,
                    InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                    InstanceDataStepRate: 0,
                },
                D3D11_INPUT_ELEMENT_DESC {
                    SemanticName: windows::core::PCSTR(b"TEXCOORD\0".as_ptr()),
                    SemanticIndex: 0,
                    Format: DXGI_FORMAT_R32G32_FLOAT,
                    InputSlot: 0,
                    AlignedByteOffset: 8,
                    InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
                    InstanceDataStepRate: 0,
                },
            ];
            let mut input_layout: Option<ID3D11InputLayout> = None;
            d3d_device
                .CreateInputLayout(&input_element_descs, vs_bytecode, Some(&mut input_layout))
                .ok()
                .context("Failed to create input layout")?;

            // Create sampler state for bilinear filtering
            let sampler_desc = windows::Win32::Graphics::Direct3D11::D3D11_SAMPLER_DESC {
                Filter: windows::Win32::Graphics::Direct3D11::D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                AddressU: windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE_ADDRESS_CLAMP,
                MipLODBias: 0.0,
                MaxAnisotropy: 1,
                ComparisonFunc: windows::Win32::Graphics::Direct3D11::D3D11_COMPARISON_NEVER,
                BorderColor: [0.0; 4],
                MinLOD: 0.0,
                MaxLOD: f32::MAX,
            };
            let mut sampler: Option<ID3D11SamplerState> = None;
            d3d_device
                .CreateSamplerState(&sampler_desc, Some(&mut sampler))
                .ok()
                .context("Failed to create sampler state")?;

            // Create vertex buffer for fullscreen quad (position x,y + texcoord u,v)
            let vertices: [Vertex; 4] = [
                Vertex {
                    x: -1.0,
                    y: -1.0,
                    u: 0.0,
                    v: 1.0,
                }, // Bottom-left (D3D Y is up, texture Y is down)
                Vertex {
                    x: -1.0,
                    y: 1.0,
                    u: 0.0,
                    v: 0.0,
                }, // Top-left
                Vertex {
                    x: 1.0,
                    y: -1.0,
                    u: 1.0,
                    v: 1.0,
                }, // Bottom-right
                Vertex {
                    x: 1.0,
                    y: 1.0,
                    u: 1.0,
                    v: 0.0,
                }, // Top-right
            ];
            let vertex_buffer_desc = windows::Win32::Graphics::Direct3D11::D3D11_BUFFER_DESC {
                ByteWidth: std::mem::size_of::<Vertex>() as u32 * 4,
                Usage: D3D11_USAGE_IMMUTABLE,
                BindFlags: D3D11_BIND_VERTEX_BUFFER.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
                StructureByteStride: 0,
            };
            let vertex_data = windows::Win32::Graphics::Direct3D11::D3D11_SUBRESOURCE_DATA {
                pSysMem: vertices.as_ptr() as *const _,
                SysMemPitch: 0,
                SysMemSlicePitch: 0,
            };
            let mut vertex_buffer: Option<ID3D11Buffer> = None;
            d3d_device
                .CreateBuffer(
                    &vertex_buffer_desc,
                    Some(&vertex_data),
                    Some(&mut vertex_buffer),
                )
                .ok()
                .context("Failed to create vertex buffer")?;

            Ok((
                vertex_shader,
                pixel_shader,
                input_layout,
                sampler,
                vertex_buffer,
            ))
        }
    }
}

/// DXGI-based screen capture
pub struct DxgiCapture {
    pub(super) config: CaptureConfig,
    pub(super) running: Arc<AtomicBool>,
    pub(super) _frame_tx: Sender<CapturedFrame>,
    pub(super) frame_rx: Receiver<CapturedFrame>,
    pub(super) fatal_tx: Sender<String>,
    pub(super) fatal_rx: Receiver<String>,
    pub(super) capture_thread: Option<JoinHandle<()>>,
    /// Cached result of NV12 conversion capability check
    nv12_conversion_capable: bool,
}

impl DxgiCapture {
    /// Create a new DXGI capture instance
    pub fn new() -> Result<Self> {
        // 32 frames at 1920x1080 BGRA is ~253MB worst case, but GPU NV12 transport greatly
        // reduces steady-state memory usage. The extra queue depth helps absorb encoder jitter
        // without immediately oscillating into adaptive throttling.
        let (frame_tx, frame_rx) = bounded::<CapturedFrame>(32);
        let (fatal_tx, fatal_rx) = bounded::<String>(8);

        // Pre-validate NV12 conversion capability
        let nv12_conversion_capable = Self::validate_nv12_capability().unwrap_or_else(|e| {
            warn!("NV12 conversion capability check failed: {}", e);
            false
        });

        if nv12_conversion_capable {
            info!("NV12 GPU conversion capability validated successfully");
        } else {
            info!("NV12 GPU conversion not available - will use CPU readback or BGRA GPU frames");
        }

        Ok(Self {
            config: CaptureConfig::default(),
            running: Arc::new(AtomicBool::new(false)),
            _frame_tx: frame_tx,
            frame_rx,
            fatal_tx,
            fatal_rx,
            capture_thread: None,
            nv12_conversion_capable,
        })
    }

    pub fn refresh_nv12_conversion_capability(&mut self, output_index: u32) -> bool {
        let nv12_conversion_capable = Self::validate_nv12_capability_for_output(output_index)
            .unwrap_or_else(|e| {
                warn!(
                    "NV12 conversion capability check failed for output {}: {}",
                    output_index, e
                );
                false
            });

        self.nv12_conversion_capable = nv12_conversion_capable;
        nv12_conversion_capable
    }

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
    fn init_capture(output_index: u32) -> Result<DxgiCaptureState> {
        DxgiCaptureState::init_capture_with_scaling(output_index, None)
    }

    /// Initialize D3D11 device and DXGI duplication with target resolution for GPU scaling
    fn init_capture_with_target(
        output_index: u32,
        target_resolution: (u32, u32),
    ) -> Result<DxgiCaptureState> {
        DxgiCaptureState::init_capture_with_scaling(output_index, Some(target_resolution))
    }

    /// Create or resize staging texture for frame readback
    /// When GPU scaling is enabled, staging texture is at target resolution
    fn ensure_staging_texture(state: &mut DxgiCaptureState) -> Result<()> {
        unsafe {
            if state.staging_texture.is_some() {
                return Ok(());
            }
            // When GPU scaling, staging texture is at target resolution (smaller)
            // When no scaling, staging texture is at native resolution
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

    /// Perform GPU-side downscaling using pixel shader
    /// This renders the captured texture to a smaller render target
    fn perform_gpu_scale(
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
            let vb = Some(vertex_buffer.clone());
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
                .PSSetSamplers(0, Some(&[Some(sampler.clone())]));
            state
                .d3d_context
                .PSSetShaderResources(0, Some(&[Some(srv)]));

            // Set render target
            state
                .d3d_context
                .OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);

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

    /// Capture a single frame
    fn capture_frame(
        state: &mut DxgiCaptureState,
        timeout_ms: u32,
        perform_cpu_readback: bool,
        _target_resolution: Option<(u32, u32)>,
    ) -> Result<Option<CapturedFrame>> {
        unsafe {
            let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut desktop_resource: Option<IDXGIResource> = None;
            let hr = state.duplication.AcquireNextFrame(
                timeout_ms,
                &mut frame_info,
                &mut desktop_resource,
            );
            match hr {
                Ok(_) => {
                    let timestamp = Self::get_qpc_timestamp();
                    let resource = desktop_resource.context("Desktop resource is null")?;
                    let captured_texture: ID3D11Texture2D = resource
                        .cast()
                        .context("Failed to cast resource to texture")?;

                    let needs_gpu_scale = state.scale_texture.is_some();

                    if needs_gpu_scale {
                        Self::perform_gpu_scale(state, &captured_texture)
                            .context("GPU scaling failed")?;
                    }

                    if !perform_cpu_readback {
                        let frame = DxgiCaptureState::capture_gpu_frame(
                            state,
                            &captured_texture,
                            timestamp,
                        )
                        .context("Failed to capture GPU frame")?;
                        state.duplication.ReleaseFrame().ok();
                        return Ok(Some(frame));
                    }

                    Self::ensure_staging_texture(state)?;
                    if let Some(ref staging) = state.staging_texture {
                        let staging_resource: ID3D11Resource = staging
                            .cast()
                            .context("Failed to cast staging texture to resource")?;

                        let (source_texture, read_resolution) =
                            DxgiCaptureState::source_texture_for_frame(state, &captured_texture)?;
                        let source_resource: ID3D11Resource = source_texture
                            .cast()
                            .context("Failed to cast source texture to resource")?;

                        state
                            .d3d_context
                            .CopyResource(Some(&staging_resource), Some(&source_resource));

                        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                        state
                            .d3d_context
                            .Map(
                                Some(&staging_resource),
                                0,
                                D3D11_MAP_READ,
                                0,
                                Some(&mut mapped as *mut D3D11_MAPPED_SUBRESOURCE),
                            )
                            .ok()
                            .context("Failed to map staging texture for readback")?;

                        let read_w = read_resolution.0 as usize;
                        let read_h = read_resolution.1 as usize;
                        let src_row_bytes = read_w * 4;
                        let src_pitch = mapped.RowPitch as usize;
                        if mapped.pData.is_null() {
                            state.d3d_context.Unmap(Some(&staging_resource), 0);
                            bail!("Mapped staging texture has null data pointer");
                        }
                        let total_src_bytes = (read_h.saturating_sub(1))
                            .checked_mul(src_pitch)
                            .and_then(|v| v.checked_add(src_row_bytes));
                        if total_src_bytes.is_none()
                            || total_src_bytes.unwrap() > isize::MAX as usize
                        {
                            state.d3d_context.Unmap(Some(&staging_resource), 0);
                            bail!(
                                "Frame dimensions too large for safe copy: {}x{}, pitch={}",
                                read_w,
                                read_h,
                                src_pitch
                            );
                        }
                        let src_ptr = mapped.pData as *const u8;
                        let total_bytes = src_row_bytes * read_h;
                        state.native_buffer.resize(total_bytes, 0);

                        if src_pitch == src_row_bytes {
                            std::ptr::copy_nonoverlapping(
                                src_ptr,
                                state.native_buffer.as_mut_ptr(),
                                total_bytes,
                            );
                        } else {
                            let dst_ptr = state.native_buffer.as_mut_ptr();
                            let mut src_row_offset = 0;
                            let mut dst_row_offset = 0;
                            for _row in 0..read_h {
                                std::ptr::copy_nonoverlapping(
                                    src_ptr.add(src_row_offset),
                                    dst_ptr.add(dst_row_offset),
                                    src_row_bytes,
                                );
                                src_row_offset += src_pitch;
                                dst_row_offset += src_row_bytes;
                            }
                        }

                        let bgra = state.native_buffer.split_to(total_bytes).freeze();
                        state.native_buffer.reserve(total_bytes);

                        state.d3d_context.Unmap(Some(&staging_resource), 0);
                        state.duplication.ReleaseFrame().ok();

                        let frame = CapturedFrame {
                            bgra,
                            d3d11: None,
                            timestamp,
                            resolution: (read_w as u32, read_h as u32),
                        };
                        return Ok(Some(frame));
                    }
                    bail!("Staging texture unavailable for CPU readback")
                }
                Err(e) if e.code().0 == DXGI_ERROR_WAIT_TIMEOUT.0 => Ok(None),
                Err(e) if e.code().0 == DXGI_ERROR_ACCESS_LOST.0 => {
                    warn!("DXGI access lost - need to reinitialize");
                    bail!("DXGI access lost")
                }
                Err(e) => bail!("AcquireNextFrame failed: 0x{:08X}", e.code().0 as u32),
            }
        }
    }

    // Manual scaler removed - replaced with FFmpeg swscale in encoder thread

    /// Capture thread entry point
    pub(crate) fn capture_loop(
        running: Arc<AtomicBool>,
        frame_tx: Sender<CapturedFrame>,
        fatal_tx: Sender<String>,
        config: CaptureConfig,
    ) {
        Self::set_capture_thread_priority();
        info!("DXGI capture thread started: {} FPS", config.target_fps);
        let perform_cpu_readback = config.perform_cpu_readback;
        let target_resolution = config.target_resolution;
        let base_fps = config.target_fps.max(1);
        let timeout_ms = (1000u32 / base_fps) * 2; // Wait up to two frames for an update before duplicating
        let frame_interval = Duration::from_nanos(1_000_000_000u64 / base_fps as u64);
        let mut next_frame_deadline = Instant::now();
        let mut frame_count = 0u64;
        let mut dropped_count = 0u64;
        let mut last_frame: Option<CapturedFrame> = None;
        let mut window_start = Instant::now();
        let mut window_frames = 0u64;
        let mut window_drops = 0u64;
        let mut error_count = 0u32;
        let max_errors = 10;
        let mut reinit_backoff_ms = 100u64;
        const MAX_BACKOFF_MS: u64 = 5000;
        let backpressure = BackpressureState::new();
        let mut adaptive_skip_counter = 0u32;
        let mut adaptive_adjust_tick = Instant::now();
        let mut pressure_high_streak = 0u32;
        let mut pressure_low_streak = 0u32;
        let mut _adaptive_level_changes = 0u64;
        // Use GPU scaling if target resolution is provided
        let mut state = match target_resolution {
            Some(res) => match Self::init_capture_with_target(config.output_index, res) {
                Ok(state) => state,
                Err(e) => {
                    error!(
                        "Failed to initialize DXGI capture with target resolution: {}",
                        e
                    );
                    let _ = fatal_tx.try_send(format!("Failed to initialize DXGI capture: {}", e));
                    return;
                }
            },
            None => match Self::init_capture(config.output_index) {
                Ok(state) => state,
                Err(e) => {
                    error!("Failed to initialize DXGI capture: {}", e);
                    let _ = fatal_tx.try_send(format!("Failed to initialize DXGI capture: {}", e));
                    return;
                }
            },
        };
        info!("DXGI capture initialized and running");
        let mut log_counter = 0u64;
        const LOG_INTERVAL: u64 = 1800; // Log every 1800 frames (~30s at 60fps)
        while running.load(Ordering::Relaxed) {
            match Self::capture_frame(
                &mut state,
                timeout_ms,
                perform_cpu_readback,
                target_resolution,
            ) {
                Ok(Some(frame)) => {
                    last_frame = Some(frame.clone());
                    let now = Instant::now();
                    if now > next_frame_deadline + Duration::from_millis(500) {
                        next_frame_deadline = now;
                    }
                    Self::wait_until_deadline(next_frame_deadline);
                    next_frame_deadline += frame_interval;
                    let fps_divisor = backpressure.current_fps_divisor();
                    if fps_divisor > 0 {
                        adaptive_skip_counter = adaptive_skip_counter.wrapping_add(1);
                        if !adaptive_skip_counter.is_multiple_of(fps_divisor + 1) {
                            dropped_count += 1;
                            window_drops += 1;
                            continue;
                        }
                    }
                    match frame_tx.try_send(frame) {
                        Ok(()) => {
                            frame_count += 1;
                            window_frames += 1;
                            error_count = 0;
                            backpressure.set_encoder_overloaded(false);
                            if frame_count % LOG_INTERVAL == 0 {
                                log_counter += 1;
                                if log_counter % 10 == 0 {
                                    info!("Captured {} frames", frame_count);
                                } else {
                                    debug!("Captured {} frames", frame_count);
                                }
                            }
                        }
                        Err(crossbeam::channel::TrySendError::Full(_frame)) => {
                            dropped_count += 1;
                            window_drops += 1;
                            error_count = 0;
                            backpressure.set_encoder_overloaded(true);
                            if dropped_count % 60 == 0 {
                                warn!("Dropped {} frames (encoder behind)", dropped_count);
                            }
                        }
                        Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
                            info!("Frame channel closed, stopping capture");
                            break;
                        }
                    }
                }
                Ok(None) => {
                    let Some(ref last) = last_frame else {
                        std::thread::sleep(Duration::from_millis(1));
                        continue;
                    };
                    let now = Instant::now();
                    if now > next_frame_deadline + Duration::from_millis(500) {
                        next_frame_deadline = now;
                    }
                    Self::wait_until_deadline(next_frame_deadline);
                    next_frame_deadline += frame_interval;
                    let fps_divisor = backpressure.current_fps_divisor();
                    if fps_divisor > 0 {
                        adaptive_skip_counter = adaptive_skip_counter.wrapping_add(1);
                        if !adaptive_skip_counter.is_multiple_of(fps_divisor + 1) {
                            dropped_count += 1;
                            window_drops += 1;
                            continue;
                        }
                    }
                    let frame = CapturedFrame {
                        bgra: if last.d3d11.is_some() {
                            Bytes::new()
                        } else {
                            last.bgra.clone()
                        },
                        d3d11: last.d3d11.clone(),
                        timestamp: Self::get_qpc_timestamp(),
                        resolution: last.resolution,
                    };
                    match frame_tx.try_send(frame) {
                        Ok(()) => {
                            frame_count += 1;
                            window_frames += 1;
                            error_count = 0;
                            backpressure.set_encoder_overloaded(false);
                        }
                        Err(crossbeam::channel::TrySendError::Full(_frame)) => {
                            dropped_count += 1;
                            window_drops += 1;
                            error_count = 0;
                            backpressure.set_encoder_overloaded(true);
                            if dropped_count % 60 == 0 {
                                warn!("Dropped {} frames (encoder behind)", dropped_count);
                            }
                        }
                        Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
                            info!("Frame channel closed, stopping capture");
                            break;
                        }
                    }
                }
                Err(e) => {
                    error!("Capture error: {}", e);
                    error_count += 1;
                    if error_count >= max_errors {
                        error!("Too many capture errors, stopping");
                        let _ = fatal_tx.try_send(format!(
                            "Capture exceeded retry budget after {} consecutive errors",
                            error_count
                        ));
                        break;
                    }
                    warn!("Attempting to reinitialize capture...");
                    let reinit_result = match target_resolution {
                        Some(res) => Self::init_capture_with_target(config.output_index, res),
                        None => Self::init_capture(config.output_index),
                    };
                    match reinit_result {
                        Ok(new_state) => {
                            state = new_state;
                            error_count = 0;
                            reinit_backoff_ms = 100;
                            info!("Reinitialization successful");
                        }
                        Err(e) => {
                            error!("Reinitialization failed: {}", e);
                            std::thread::sleep(Duration::from_millis(reinit_backoff_ms));
                            reinit_backoff_ms = (reinit_backoff_ms * 2).min(MAX_BACKOFF_MS);
                        }
                    }
                }
            }
            if adaptive_adjust_tick.elapsed() >= Duration::from_secs(2) {
                let queue_len = frame_tx.len() as u32;
                let queue_cap = frame_tx.capacity().unwrap_or(32) as u32;
                let high_watermark = queue_cap.saturating_mul(3) / 4;
                let low_watermark = queue_cap / 4;
                let severe_watermark = queue_cap.saturating_mul(7) / 8;
                let mut fps_divisor = backpressure.current_fps_divisor();
                let encoder_overloaded = backpressure.is_encoder_overloaded();

                if queue_len >= severe_watermark {
                    pressure_high_streak = 0;
                    pressure_low_streak = 0;
                    if fps_divisor < 3 {
                        fps_divisor += 1;
                        backpressure.set_fps_divisor(fps_divisor);
                        _adaptive_level_changes += 1;
                        warn!(
                            "Adaptive throttling increased: fps_divisor={} queue={}/{}",
                            fps_divisor, queue_len, queue_cap
                        );
                    }
                } else if encoder_overloaded || queue_len >= high_watermark {
                    pressure_high_streak = pressure_high_streak.saturating_add(1);
                    pressure_low_streak = 0;
                    if pressure_high_streak >= 2 && fps_divisor < 3 {
                        fps_divisor += 1;
                        backpressure.set_fps_divisor(fps_divisor);
                        _adaptive_level_changes += 1;
                        pressure_high_streak = 0;
                        warn!(
                            "Adaptive throttling increased: fps_divisor={} queue={}/{}",
                            fps_divisor, queue_len, queue_cap
                        );
                    }
                } else if queue_len <= low_watermark {
                    pressure_low_streak = pressure_low_streak.saturating_add(1);
                    pressure_high_streak = 0;
                    if pressure_low_streak >= 3 && fps_divisor > 0 {
                        fps_divisor -= 1;
                        backpressure.set_fps_divisor(fps_divisor);
                        _adaptive_level_changes += 1;
                        pressure_low_streak = 0;
                        info!(
                            "Adaptive throttling reduced: fps_divisor={} queue={}/{}",
                            fps_divisor, queue_len, queue_cap
                        );
                    }
                } else {
                    pressure_high_streak = 0;
                    pressure_low_streak = 0;
                }
                adaptive_adjust_tick = Instant::now();
            }
            if window_start.elapsed() >= Duration::from_secs(30) {
                debug!(
                    "Capture: {}fps, drops={}, divisor={}",
                    window_frames / 30,
                    window_drops,
                    backpressure.current_fps_divisor()
                );
                window_start = Instant::now();
                window_frames = 0;
                window_drops = 0;
            }
        }
        info!(
            "DXGI capture thread stopped ({} frames captured, {} dropped)",
            frame_count, dropped_count
        );
    }

    /// Get current QPC timestamp
    fn get_qpc_timestamp() -> i64 {
        DxgiCaptureState::get_qpc_timestamp()
    }

    fn wait_until_deadline(deadline: Instant) {
        const SPIN_THRESHOLD: Duration = Duration::from_millis(1);
        loop {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let remaining = deadline - now;
            if remaining > SPIN_THRESHOLD {
                std::thread::sleep(remaining - SPIN_THRESHOLD);
            } else {
                std::hint::spin_loop();
            }
        }
    }

    fn set_capture_thread_priority() {
        #[cfg(windows)]
        {
            use windows::Win32::System::Threading::{
                GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL,
            };
            unsafe {
                if let Err(e) = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL)
                {
                    warn!("Failed to raise capture thread priority: {}", e);
                }
            }
        }
    }
}
