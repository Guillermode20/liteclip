//! DXGI capture with GPU-side scaling support

use crate::capture::{backpressure::BackpressureState, CaptureConfig, CapturedFrame, D3d11Frame};
use anyhow::{bail, Context, Result};
use bytes::{Bytes, BytesMut};
use crossbeam::channel::{bounded, Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};
use windows::Win32::Foundation::BOOL;
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Buffer, ID3D11Device, ID3D11DeviceContext, ID3D11InputLayout, ID3D11Multithread,
    ID3D11PixelShader, ID3D11RenderTargetView, ID3D11Resource, ID3D11SamplerState,
    ID3D11ShaderResourceView, ID3D11Texture2D, ID3D11VertexShader, D3D11_BIND_VERTEX_BUFFER,
    D3D11_INPUT_ELEMENT_DESC, D3D11_INPUT_PER_VERTEX_DATA, D3D11_MAPPED_SUBRESOURCE,
    D3D11_MAP_READ, D3D11_USAGE_IMMUTABLE,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R32G32_FLOAT;
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

/// DXGI capture state with GPU-side scaling support
struct DxgiCaptureState {
    d3d_device: ID3D11Device,
    d3d_context: ID3D11DeviceContext,
    duplication: windows::Win32::Graphics::Dxgi::IDXGIOutputDuplication,
    #[allow(dead_code)]
    output_desc: DXGI_OUTPUT_DESC,
    /// Staging texture for CPU readback (at target resolution after GPU scaling)
    staging_texture: Option<ID3D11Texture2D>,
    /// Native capture resolution
    frame_width: u32,
    frame_height: u32,
    /// Target output resolution (for GPU scaling)
    target_width: u32,
    target_height: u32,
    /// GPU resources for shader-based scaling
    vertex_shader: Option<ID3D11VertexShader>,
    pixel_shader: Option<ID3D11PixelShader>,
    input_layout: Option<ID3D11InputLayout>,
    sampler: Option<ID3D11SamplerState>,
    vertex_buffer: Option<ID3D11Buffer>,
    /// Render target view for the scaled output texture
    rtv: Option<ID3D11RenderTargetView>,
    /// Scaled output texture (render target for GPU downscaling)
    scale_texture: Option<ID3D11Texture2D>,
    native_buffer: BytesMut,
}
impl DxgiCaptureState {
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
                            Format:
                                windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
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

    fn create_owned_texture(
        state: &DxgiCaptureState,
        width: u32,
        height: u32,
    ) -> Result<ID3D11Texture2D> {
        unsafe {
            let desc = windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: windows::Win32::Graphics::Direct3D11::D3D11_USAGE_DEFAULT,
                BindFlags: 0,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let mut texture = None;
            state
                .d3d_device
                .CreateTexture2D(&desc, None, Some(&mut texture))
                .ok()
                .context("Failed to create owned GPU frame texture")?;
            texture.context("Owned GPU frame texture is null")
        }
    }

    fn source_texture_for_frame(
        state: &DxgiCaptureState,
        captured_texture: &ID3D11Texture2D,
    ) -> Result<(ID3D11Resource, (u32, u32))> {
        let needs_gpu_scale = state.scale_texture.is_some();
        let source_texture: ID3D11Resource = if needs_gpu_scale {
            state
                .scale_texture
                .as_ref()
                .context("Scale texture is None")?
                .cast()
                .context("Failed to cast scale texture to resource")?
        } else {
            captured_texture
                .cast()
                .context("Failed to cast captured texture to resource")?
        };
        let resolution = if needs_gpu_scale {
            (state.target_width, state.target_height)
        } else {
            (state.frame_width, state.frame_height)
        };
        Ok((source_texture, resolution))
    }

    fn capture_gpu_frame(
        state: &mut DxgiCaptureState,
        captured_texture: &ID3D11Texture2D,
        timestamp: i64,
    ) -> Result<CapturedFrame> {
        unsafe {
            let (source_texture, resolution) =
                Self::source_texture_for_frame(state, captured_texture)?;
            let owned_texture = Self::create_owned_texture(state, resolution.0, resolution.1)?;
            let owned_resource: ID3D11Resource = owned_texture
                .cast()
                .context("Failed to cast owned GPU frame texture to resource")?;
            state
                .d3d_context
                .CopyResource(Some(&owned_resource), Some(&source_texture));

            Ok(CapturedFrame {
                bgra: Bytes::new(),
                d3d11: Some(Arc::new(D3d11Frame {
                    texture: owned_texture,
                    device: state.d3d_device.clone(),
                })),
                timestamp,
                resolution,
            })
        }
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
}
impl DxgiCapture {
    /// Create a new DXGI capture instance
    pub fn new() -> Result<Self> {
        // 16 frames at 2560x1440 BGRA = ~235MB (provides ~267ms buffer at 60fps)
        // This gives the encoder enough headroom to handle transient slowdowns
        let (frame_tx, frame_rx) = bounded::<CapturedFrame>(16);
        let (fatal_tx, fatal_rx) = bounded::<String>(8);
        Ok(Self {
            config: CaptureConfig::default(),
            running: Arc::new(AtomicBool::new(false)),
            _frame_tx: frame_tx,
            frame_rx,
            fatal_tx,
            fatal_rx,
            capture_thread: None,
        })
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
                Format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
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

                    // Check if we need GPU scaling
                    let needs_gpu_scale = state.scale_texture.is_some();

                    if needs_gpu_scale {
                        // Perform GPU-side downscaling using pixel shader
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

                        state
                            .d3d_context
                            .CopyResource(Some(&staging_resource), Some(&source_texture));

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

                        // Read back at target resolution (smaller if GPU scaling)
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
                        bgra: last.bgra.clone(),
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
                let mut fps_divisor = backpressure.current_fps_divisor();
                if backpressure.is_encoder_overloaded() || queue_len >= high_watermark {
                    if fps_divisor < 3 {
                        fps_divisor += 1;
                        backpressure.set_fps_divisor(fps_divisor);
                        _adaptive_level_changes += 1;
                        warn!(
                            "Adaptive throttling increased: fps_divisor={} queue={}/{}",
                            fps_divisor, queue_len, queue_cap
                        );
                    }
                } else if queue_len <= low_watermark && fps_divisor > 0 {
                    fps_divisor -= 1;
                    backpressure.set_fps_divisor(fps_divisor);
                    _adaptive_level_changes += 1;
                    info!(
                        "Adaptive throttling reduced: fps_divisor={} queue={}/{}",
                        fps_divisor, queue_len, queue_cap
                    );
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
