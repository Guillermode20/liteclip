//! DXGI capture with GPU-side scaling support

use crate::capture::{
    backpressure::BackpressureState, CaptureConfig, CapturedFrame, D3d11TexturePoolItem,
};
use anyhow::{bail, Context, Result};
use bytes::{Bytes, BytesMut};
use crossbeam::channel::{bounded, unbounded, Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};
use windows::Win32::Foundation::{BOOL, HANDLE};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Buffer, ID3D11Device, ID3D11Device5, ID3D11DeviceContext, ID3D11Fence, ID3D11InputLayout,
    ID3D11Multithread, ID3D11PixelShader, ID3D11RenderTargetView, ID3D11Resource,
    ID3D11SamplerState, ID3D11Texture2D, ID3D11VertexShader, ID3D11VideoContext, ID3D11VideoDevice,
    ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator, ID3D11VideoProcessorInputView,
    D3D11_BIND_VERTEX_BUFFER, D3D11_FENCE_FLAG_SHARED, D3D11_INPUT_ELEMENT_DESC,
    D3D11_INPUT_PER_VERTEX_DATA, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_USAGE_IMMUTABLE,
    D3D11_VIDEO_PROCESSOR_COLOR_SPACE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_FORMAT_R32G32_FLOAT, DXGI_RATIONAL,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIOutput1, IDXGIResource, DXGI_ERROR_ACCESS_DENIED,
    DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_INVALID_CALL, DXGI_ERROR_NON_COMPOSITED_UI,
    DXGI_ERROR_NOT_CURRENTLY_AVAILABLE, DXGI_ERROR_UNSUPPORTED, DXGI_ERROR_WAIT_TIMEOUT,
    DXGI_OUTDUPL_FRAME_INFO,
};
use windows::Win32::System::Performance::QueryPerformanceCounter;
use windows_core::Interface;

/// Simple vertex for fullscreen quad: position (x, y) and texcoord (u, v)
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct Vertex {
    x: f32,
    y: f32,
    u: f32,
    v: f32,
}

pub(super) struct Nv12TexturePool {
    pub(super) available: Vec<D3d11TexturePoolItem>,
    pub(super) return_tx: Sender<D3d11TexturePoolItem>,
    pub(super) return_rx: Receiver<D3d11TexturePoolItem>,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) max_capacity: usize,
}

pub(super) struct BgraTexturePool {
    pub(super) available: Vec<D3d11TexturePoolItem>,
    pub(super) return_tx: Sender<D3d11TexturePoolItem>,
    pub(super) return_rx: Receiver<D3d11TexturePoolItem>,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) max_capacity: usize,
}

enum CaptureOutcome {
    Frame(CapturedFrame),
    Timeout,
    Dropped,
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
            max_capacity: 12, // Sufficient for jitter but caps VRAM usage
        }
    }
}

impl BgraTexturePool {
    fn new(width: u32, height: u32) -> Self {
        let (return_tx, return_rx) = unbounded();
        Self {
            available: Vec::new(),
            return_tx,
            return_rx,
            width,
            height,
            max_capacity: 12, // Sufficient for jitter but caps VRAM usage
        }
    }
}

/// DXGI capture state with GPU-side scaling support
pub(super) struct DxgiCaptureState {
    pub(super) d3d_device: ID3D11Device,
    pub(super) d3d_context: ID3D11DeviceContext,
    pub(super) duplication: windows::Win32::Graphics::Dxgi::IDXGIOutputDuplication,
    pub(super) staging_texture: Option<ID3D11Texture2D>,
    pub(super) frame_width: u32,
    pub(super) frame_height: u32,
    pub(super) target_width: u32,
    pub(super) target_height: u32,
    pub(super) vertex_shader: Option<ID3D11VertexShader>,
    pub(super) pixel_shader: Option<ID3D11PixelShader>,
    pub(super) input_layout: Option<ID3D11InputLayout>,
    pub(super) sampler: Option<ID3D11SamplerState>,
    pub(super) vertex_buffer: Option<ID3D11Buffer>,
    pub(super) rtv: Option<ID3D11RenderTargetView>,
    pub(super) scale_texture: Option<ID3D11Texture2D>,
    pub(super) native_buffer: BytesMut,
    pub(super) bgra_pool: Option<BgraTexturePool>,
    pub(super) nv12_pool: Option<Nv12TexturePool>,
    pub(super) video_device: Option<ID3D11VideoDevice>,
    pub(super) video_context: Option<ID3D11VideoContext>,
    pub(super) video_processor: Option<ID3D11VideoProcessor>,
    pub(super) video_processor_enumerator: Option<ID3D11VideoProcessorEnumerator>,
    pub(super) scale_input_view: Option<ID3D11VideoProcessorInputView>,
    pub(super) nv12_conversion_available: bool,
    pub(super) nv12_runtime_failures: u32,
    pub(super) nv12_retry_after: Option<Instant>,
    pub(super) nv12_unavailable_logged: bool,
    /// Shared ID3D11Fence used for GPU-side cross-device synchronization.
    /// After VideoProcessorBlt the capture GPU queue signals this fence; the encoder GPU
    /// queue waits on it before CopySubresourceRegion. No CPU stall — ordering is fully on the GPU.
    pub(super) nv12_sync_fence: Option<ID3D11Fence>,
    /// NT kernel handle for `nv12_sync_fence` (D3D11_FENCE_FLAG_SHARED).
    /// Passed through D3d11Frame so the encoder can call OpenSharedFence once on its own device.
    pub(super) nv12_fence_shared_handle: Option<HANDLE>,
    /// Monotonically increasing value. Incremented and signaled before each frame hand-off.
    pub(super) nv12_fence_value: u64,
    pub(super) bgra_sync_fence: Option<ID3D11Fence>,
    pub(super) bgra_fence_shared_handle: Option<HANDLE>,
    pub(super) bgra_fence_value: u64,
}

impl DxgiCaptureState {
    pub(super) fn video_processor_color_space(
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
    pub(super) fn init_capture_with_scaling(
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
                            if let Some(tex) = texture.as_ref() {
                                if d3d_device
                                    .CreateRenderTargetView(tex, None, Some(&mut rtv))
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
            ) = Self::init_nv12_conversion_resources(
                &d3d_device,
                frame_width,
                frame_height,
                target_width,
                target_height,
            )
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

            let mut bgra_pool = BgraTexturePool::new(target_width, target_height);
            for _ in 0..4 {
                bgra_pool.available.push(DxgiCapture::create_bgra_pool_item(
                    &d3d_device,
                    target_width,
                    target_height,
                )?);
            }

            let (bgra_sync_fence, bgra_fence_shared_handle) = match d3d_device
                .cast::<ID3D11Device5>()
            {
                Ok(device5) => {
                    let mut fence_opt: Option<ID3D11Fence> = None;
                    match device5.CreateFence(0, D3D11_FENCE_FLAG_SHARED, &mut fence_opt) {
                        Ok(()) => match fence_opt {
                            Some(fence) => match fence.CreateSharedHandle(
                                None,
                                0x10000000u32,
                                windows_core::PCWSTR::null(),
                            ) {
                                Ok(handle) => {
                                    info!("BGRA sync fence created for cross-device GPU copies");
                                    (Some(fence), Some(handle))
                                }
                                Err(e) => {
                                    warn!("Failed to create shared BGRA fence handle, BGRA sync will fall back to Flush-only: {}", e);
                                    (None, None)
                                }
                            },
                            None => {
                                warn!("CreateFence returned null, BGRA sync will fall back to Flush-only");
                                (None, None)
                            }
                        },
                        Err(e) => {
                            warn!("Failed to create BGRA sync fence, BGRA sync will fall back to Flush-only: {}", e);
                            (None, None)
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        "ID3D11Device5 unavailable, BGRA sync will fall back to Flush-only: {}",
                        e
                    );
                    (None, None)
                }
            };

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
                bgra_pool: Some(bgra_pool),
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
                bgra_sync_fence,
                bgra_fence_shared_handle,
                bgra_fence_value: 0,
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
    #[allow(clippy::type_complexity)]
    fn init_nv12_conversion_resources(
        d3d_device: &ID3D11Device,
        input_width: u32,
        input_height: u32,
        output_width: u32,
        output_height: u32,
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
                InputWidth: input_width,
                InputHeight: input_height,
                OutputFrameRate: DXGI_RATIONAL {
                    Numerator: 60,
                    Denominator: 1,
                },
                OutputWidth: output_width,
                OutputHeight: output_height,
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

            let mut nv12_pool = Nv12TexturePool::new(output_width, output_height);
            for _ in 0..4 {
                nv12_pool.available.push(DxgiCapture::create_nv12_pool_item(
                    d3d_device,
                    &video_device,
                    &video_processor_enumerator,
                    output_width,
                    output_height,
                )?);
            }

            info!(
                "NV12 video processor initialized successfully for {}x{} -> {}x{}",
                input_width, input_height, output_width, output_height
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

    /// Initialize GPU scaling resources (shaders, vertex buffer)
    #[allow(clippy::type_complexity)]
    #[allow(clippy::manual_c_str_literals)]
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

/// DXGI-based screen capture implementation.
///
/// Captures screen content using Windows DXGI Desktop Duplication API.
/// Runs on a dedicated thread, producing captured frames via a channel.
///
/// # Features
///
/// - GPU-accelerated capture via DXGI
/// - GPU-side scaling and format conversion (NV12)
/// - Cross-device texture sharing for zero-copy encoding
/// - Frame queue with configurable depth
///
/// # Thread Safety
///
/// The capture runs on its own thread. The public methods for start/stop
/// can be called from any thread, but typically from the main application.
pub struct DxgiCapture {
    /// Capture configuration.
    pub(super) config: CaptureConfig,
    /// Atomic flag indicating if capture is running.
    pub(super) running: Arc<AtomicBool>,
    /// Channel sender for captured frames.
    pub(super) _frame_tx: Sender<CapturedFrame>,
    /// Channel receiver for captured frames.
    pub(super) frame_rx: Receiver<CapturedFrame>,
    /// Channel for fatal error reporting.
    pub(super) fatal_tx: Sender<String>,
    /// Receiver for fatal errors.
    pub(super) fatal_rx: Receiver<String>,
    /// Handle to the capture thread.
    pub(super) capture_thread: Option<JoinHandle<()>>,
    /// Cached result of NV12 conversion capability check.
    pub(super) nv12_conversion_capable: bool,
}

impl DxgiCapture {
    /// Creates a new DXGI capture instance.
    ///
    /// # Errors
    ///
    /// Returns an error if DXGI initialization fails.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use liteclip_replay::capture::dxgi::DxgiCapture;
    ///
    /// let capture = DxgiCapture::new().unwrap();
    /// ```
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

    /// Capture a single frame
    fn capture_frame(
        state: &mut DxgiCaptureState,
        timeout_ms: u32,
        drop_before_process: bool,
        perform_cpu_readback: bool,
        #[cfg(windows)] gpu_texture_format: crate::capture::GpuTextureFormat,
        _target_resolution: Option<(u32, u32)>,
    ) -> Result<CaptureOutcome> {
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
                    if drop_before_process {
                        state.duplication.ReleaseFrame().ok();
                        return Ok(CaptureOutcome::Dropped);
                    }
                    // Use capture-time QPC for replay timestamps. `LastPresentTime` reflects the
                    // desktop compositor's last present event, which can lag or remain stale across
                    // duplication events and causes saved clips to collapse in duration.
                    let timestamp = Self::get_qpc_timestamp();
                    let resource = desktop_resource.context("Desktop resource is null")?;
                    let captured_texture: ID3D11Texture2D = resource
                        .cast()
                        .context("Failed to cast resource to texture")?;

                    let needs_gpu_scale = state.scale_texture.is_some();
                    let needs_separate_gpu_scale = needs_gpu_scale
                        && (perform_cpu_readback
                            || gpu_texture_format != crate::capture::GpuTextureFormat::Nv12
                            || !state.nv12_conversion_available);

                    if needs_separate_gpu_scale {
                        Self::perform_gpu_scale(state, &captured_texture)
                            .context("GPU scaling failed")?;
                    }

                    if !perform_cpu_readback {
                        let frame = DxgiCapture::capture_gpu_frame(
                            state,
                            &captured_texture,
                            timestamp,
                            gpu_texture_format,
                        )?;
                        state.duplication.ReleaseFrame().ok();
                        return Ok(CaptureOutcome::Frame(frame));
                    }

                    Self::ensure_staging_texture(state)?;
                    if let Some(ref staging) = state.staging_texture {
                        let staging_resource: ID3D11Resource = staging
                            .cast()
                            .context("Failed to cast staging texture to resource")?;

                        let (source_texture, resolution) =
                            DxgiCapture::source_texture_for_frame(state, &captured_texture, None)?;
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

                        let read_w = resolution.0 as usize;
                        let read_h = resolution.1 as usize;
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
                        return Ok(CaptureOutcome::Frame(frame));
                    }
                    bail!("Staging texture unavailable for CPU readback")
                }
                Err(e) if e.code().0 == DXGI_ERROR_WAIT_TIMEOUT.0 => Ok(CaptureOutcome::Timeout),
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
    /// The main capture loop for DXGI Desktop Duplication.
    ///
    /// This function runs in a dedicated thread and performs the following:
    /// 1. Initializes DXGI and D3D11 resources for the target monitor.
    /// 2. Enters a high-frequency loop to acquire frames at the target FPS.
    /// 3. Handles frame capture, CPU readback (if configured), and GPU scaling/format conversion.
    /// 4. Manages errors and automatic re-initialization with exponential backoff if the session is lost.
    /// 5. Reports fatal errors via the `fatal_tx` channel.
    ///
    /// # Arguments
    ///
    /// * `running` - Atomic flag to control the loop lifetime.
    /// * `frame_tx` - Channel to send captured frames to the encoder.
    /// * `fatal_tx` - Channel to report unrecoverable errors.
    /// * `config` - Capture configuration (FPS, resolution, format).
    pub(crate) fn capture_loop(
        running: Arc<AtomicBool>,
        frame_tx: Sender<CapturedFrame>,
        fatal_tx: Sender<String>,
        config: CaptureConfig,
    ) {
        Self::set_capture_thread_priority();

        // Use high-resolution timer for precise sleep on Windows
        unsafe {
            let _ = windows::Win32::Media::timeBeginPeriod(1);
        }

        info!("DXGI capture thread started: {} FPS", config.target_fps);
        let perform_cpu_readback = config.perform_cpu_readback;
        #[cfg(windows)]
        let gpu_texture_format = config.gpu_texture_format;
        let target_resolution = config.target_resolution;
        let base_fps = config.target_fps.max(1);
        let timeout_ms = (1000u32.saturating_add(base_fps).saturating_sub(1) / base_fps).max(1);
        let mut frame_count = 0u64;
        let mut dropped_count = 0u64;
        let mut duplicated_count = 0u64;
        let mut last_frame: Option<CapturedFrame> = None;
        let mut window_start = Instant::now();
        let mut window_frames = 0u64;
        let mut window_drops = 0u64;
        let mut window_duplicates = 0u64;
        let mut error_count = 0u32;
        let max_errors = 10;
        let mut reinit_backoff_ms = 100u64;
        const MAX_BACKOFF_MS: u64 = 5000;
        let backpressure = BackpressureState::new();
        let mut adaptive_skip_counter = 0u32;
        let mut adaptive_adjust_tick = Instant::now();
        let mut pressure_high_streak = 0u32;
        let mut pressure_low_streak = 0u32;
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
        const DROP_LOG_INTERVAL: u64 = 300;
        const FRAME_PACING_SPIN_WINDOW: Duration = Duration::from_micros(200);
        let frame_period = Duration::from_nanos(1_000_000_000u64 / base_fps as u64);
        let mut next_frame_time = std::time::Instant::now();
        while running.load(Ordering::Relaxed) {
            let mut drop_before_process = false;
            let queue_len = frame_tx.len() as u32;
            let queue_cap = frame_tx.capacity().unwrap_or(32) as u32;
            let fps_divisor = backpressure.current_fps_divisor();
            if fps_divisor > 0 {
                adaptive_skip_counter = adaptive_skip_counter.wrapping_add(1);
                drop_before_process = !adaptive_skip_counter.is_multiple_of(fps_divisor + 1);
            }
            if !drop_before_process && queue_len.saturating_add(1) >= queue_cap {
                drop_before_process = true;
            }
            match Self::capture_frame(
                &mut state,
                timeout_ms,
                drop_before_process,
                perform_cpu_readback,
                #[cfg(windows)]
                gpu_texture_format,
                target_resolution,
            ) {
                Ok(CaptureOutcome::Frame(frame)) => {
                    // Always store the last frame so the timeout path can send
                    // duplicate frames while DXGI has no new desktop update.
                    // GPU frames use Arc<D3d11Frame>, so cloning is safe — the
                    // pooled NV12 texture is returned to the pool only after every
                    // clone has been dropped.
                    last_frame = Some(frame.clone());
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
                            if dropped_count % DROP_LOG_INTERVAL == 0 {
                                warn!("Dropped {} frames (encoder behind)", dropped_count);
                            }
                        }
                        Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
                            info!("Frame channel closed, stopping capture");
                            break;
                        }
                    }
                }
                Ok(CaptureOutcome::Dropped) => {
                    dropped_count += 1;
                    window_drops += 1;
                    error_count = 0;
                }
                Ok(CaptureOutcome::Timeout) => {
                    let Some(ref last) = last_frame else {
                        continue;
                    };
                    if backpressure.is_encoder_overloaded() || !frame_tx.is_empty() {
                        continue;
                    }
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
                        // GPU frames are wrapped in `Arc<D3d11Frame>`, so re-sending the most
                        // recent frame on DXGI timeouts is safe. The pooled texture is only
                        // recycled once the final clone is dropped by the encoder.
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
                            duplicated_count += 1;
                            window_frames += 1;
                            window_duplicates += 1;
                            error_count = 0;
                            backpressure.set_encoder_overloaded(false);
                        }
                        Err(crossbeam::channel::TrySendError::Full(_frame)) => {
                            dropped_count += 1;
                            window_drops += 1;
                            error_count = 0;
                            backpressure.set_encoder_overloaded(true);
                            if dropped_count % DROP_LOG_INTERVAL == 0 {
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
                    "Capture: {}fps, drops={}, duplicates={}, divisor={}",
                    window_frames / 30,
                    window_drops,
                    window_duplicates,
                    backpressure.current_fps_divisor()
                );
                window_start = Instant::now();
                window_frames = 0;
                window_drops = 0;
                window_duplicates = 0;
            }

            // Precise pacing: mostly sleep, then a short final spin for accuracy.
            let now = std::time::Instant::now();
            if now < next_frame_time {
                let diff = next_frame_time - now;
                if diff > FRAME_PACING_SPIN_WINDOW {
                    std::thread::sleep(diff - FRAME_PACING_SPIN_WINDOW);
                }
                while std::time::Instant::now() < next_frame_time {
                    std::hint::spin_loop();
                }
            }
            next_frame_time += frame_period;
            // If we're significantly behind, reset the schedule
            if std::time::Instant::now() > next_frame_time + frame_period {
                next_frame_time = std::time::Instant::now() + frame_period;
            }
        }

        unsafe {
            let _ = windows::Win32::Media::timeEndPeriod(1);
        }

        info!(
            "DXGI capture thread stopped ({} frames captured, {} dropped, {} duplicated)",
            frame_count, dropped_count, duplicated_count
        );
    }

    /// Get current QPC timestamp
    fn get_qpc_timestamp() -> i64 {
        DxgiCaptureState::get_qpc_timestamp()
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
