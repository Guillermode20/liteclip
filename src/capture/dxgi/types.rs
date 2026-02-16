//! Auto-generated module
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use windows_core::Interface;

use crate::capture::{CaptureConfig, CapturedFrame};
use crate::d3d::D3D11Texture;
use anyhow::{bail, Context, Result};
use bytes::BytesMut;
use crossbeam::channel::{bounded, Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;
use tracing::{debug, error, info, warn};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11Texture2D, D3D11_MAPPED_SUBRESOURCE,
    D3D11_MAP_READ,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIOutput1, IDXGIResource, DXGI_ERROR_ACCESS_DENIED,
    DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_INVALID_CALL, DXGI_ERROR_NON_COMPOSITED_UI,
    DXGI_ERROR_NOT_CURRENTLY_AVAILABLE, DXGI_ERROR_UNSUPPORTED, DXGI_ERROR_WAIT_TIMEOUT,
    DXGI_OUTDUPL_FRAME_INFO, DXGI_OUTPUT_DESC,
};
use windows::Win32::System::Performance::QueryPerformanceCounter;

/// DXGI capture state
#[allow(dead_code)]
struct DxgiCaptureState {
    d3d_device: ID3D11Device,
    d3d_context: ID3D11DeviceContext,
    duplication: windows::Win32::Graphics::Dxgi::IDXGIOutputDuplication,
    #[allow(dead_code)]
    output_desc: DXGI_OUTPUT_DESC,
    staging_texture: Option<ID3D11Texture2D>,
    frame_width: u32,
    frame_height: u32,
}
impl DxgiCaptureState {
    pub fn get_qpc_timestamp() -> i64 {
        unsafe {
            let mut qpc = 0i64;
            QueryPerformanceCounter(&mut qpc).expect("QueryPerformanceCounter should never fail");
            qpc
        }
    }
}
/// DXGI-based screen capture
pub struct DxgiCapture {
    pub(super) config: CaptureConfig,
    pub(super) running: Arc<AtomicBool>,
    pub(super) _frame_tx: Sender<CapturedFrame>,
    pub(super) frame_rx: Receiver<CapturedFrame>,
    pub(super) capture_thread: Option<JoinHandle<()>>,
}
impl DxgiCapture {
    /// Create a new DXGI capture instance
    pub fn new() -> Result<Self> {
        let (frame_tx, frame_rx) = bounded::<CapturedFrame>(256);
        Ok(Self {
            config: CaptureConfig::default(),
            running: Arc::new(AtomicBool::new(false)),
            _frame_tx: frame_tx,
            frame_rx,
            capture_thread: None,
        })
    }
    /// Initialize D3D11 device and DXGI duplication
    fn init_capture(output_index: u32) -> Result<DxgiCaptureState> {
        info!("Initializing DXGI capture for output {}", output_index);
        unsafe {
            let factory = CreateDXGIFactory1::<windows::Win32::Graphics::Dxgi::IDXGIFactory1>()
                .context("Failed to create DXGI factory")?;
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
            let output_desc = output
                .GetDesc()
                .context("Failed to get output description")?;
            info!(
                "Using output: {}x{} attached to monitor {:?}",
                output_desc.DesktopCoordinates.right - output_desc.DesktopCoordinates.left,
                output_desc.DesktopCoordinates.bottom - output_desc.DesktopCoordinates.top,
                output_desc.Monitor
            );
            let output1: IDXGIOutput1 = output
                .cast()
                .context("Failed to get IDXGIOutput1 interface")?;
            let adapter_cast: windows::Win32::Graphics::Dxgi::IDXGIAdapter =
                adapter.cast().context("Failed to cast adapter")?;
            let mut d3d_device: Option<ID3D11Device> = None;
            let mut d3d_context: Option<ID3D11DeviceContext> = None;
            let feature_levels = [windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0];
            let mut obtained_feature_level =
                windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL(0);
            let result = windows::Win32::Graphics::Direct3D11::D3D11CreateDevice(
                Some(&adapter_cast),
                windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN,
                windows::Win32::Foundation::HINSTANCE(std::ptr::null_mut()),
                windows::Win32::Graphics::Direct3D11::D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&feature_levels),
                windows::Win32::Graphics::Direct3D11::D3D11_SDK_VERSION,
                Some(&mut d3d_device as *mut Option<ID3D11Device>),
                Some(
                    &mut obtained_feature_level
                        as *mut windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL,
                ),
                Some(&mut d3d_context as *mut Option<ID3D11DeviceContext>),
            );
            result.ok().context("Failed to create D3D11 device")?;
            let d3d_device = d3d_device.context("D3D11 device is null")?;
            let d3d_context = d3d_context.context("D3D11 context is null")?;
            let duplication = output1
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
            info!("DXGI capture initialized: {}x{}", frame_width, frame_height);
            Ok(DxgiCaptureState {
                d3d_device,
                d3d_context,
                duplication,
                output_desc,
                staging_texture: None,
                frame_width,
                frame_height,
            })
        }
    }
    /// Create or resize staging texture for frame readback
    fn ensure_staging_texture(state: &mut DxgiCaptureState) -> Result<()> {
        unsafe {
            if state.staging_texture.is_some() {
                return Ok(());
            }
            let desc = windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC {
                Width: state.frame_width,
                Height: state.frame_height,
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
            debug!(
                "Created staging texture: {}x{}",
                state.frame_width, state.frame_height
            );
            Ok(())
        }
    }
    /// Capture a single frame
    fn capture_frame(
        state: &mut DxgiCaptureState,
        timeout_ms: u32,
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
                    let resource = desktop_resource.context("Desktop resource is null")?;
                    let captured_texture: ID3D11Texture2D = resource
                        .cast()
                        .context("Failed to cast resource to texture")?;
                    Self::ensure_staging_texture(state)?;
                    if let Some(ref staging) = state.staging_texture {
                        let staging_resource: ID3D11Resource = staging
                            .cast()
                            .context("Failed to cast staging texture to resource")?;
                        let captured_resource: ID3D11Resource = captured_texture
                            .cast()
                            .context("Failed to cast captured texture to resource")?;
                        state
                            .d3d_context
                            .CopyResource(Some(&staging_resource), Some(&captured_resource));
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
                        let width = state.frame_width as usize;
                        let height = state.frame_height as usize;
                        let row_bytes = width * 4;
                        let src_pitch = mapped.RowPitch as usize;
                        if mapped.pData.is_null() {
                            state.d3d_context.Unmap(Some(&staging_resource), 0);
                            bail!("Mapped staging texture has null data pointer");
                        }
                        let total_src_bytes = (height.saturating_sub(1))
                            .checked_mul(src_pitch)
                            .and_then(|v| v.checked_add(row_bytes));
                        if total_src_bytes.is_none()
                            || total_src_bytes.unwrap() > isize::MAX as usize
                        {
                            state.d3d_context.Unmap(Some(&staging_resource), 0);
                            bail!(
                                "Frame dimensions too large for safe copy: {}x{}, pitch={}",
                                width,
                                height,
                                src_pitch
                            );
                        }
                        let total_bytes = row_bytes * height;
                        let src_ptr = mapped.pData as *const u8;
                        let mut bgra_buffer = BytesMut::with_capacity(total_bytes);
                        bgra_buffer.set_len(total_bytes);
                        if src_pitch == row_bytes {
                            std::ptr::copy_nonoverlapping(
                                src_ptr,
                                bgra_buffer.as_mut_ptr(),
                                total_bytes,
                            );
                        } else {
                            let dst_ptr = bgra_buffer.as_mut_ptr();
                            let mut src_row_offset = 0;
                            let mut dst_row_offset = 0;
                            for _row in 0..height {
                                std::ptr::copy_nonoverlapping(
                                    src_ptr.add(src_row_offset),
                                    dst_ptr.add(dst_row_offset),
                                    row_bytes,
                                );
                                src_row_offset += src_pitch;
                                dst_row_offset += row_bytes;
                            }
                        }
                        let bgra = bgra_buffer.freeze();
                        state.d3d_context.Unmap(Some(&staging_resource), 0);
                        state.duplication.ReleaseFrame().ok();
                        let timestamp = Self::get_qpc_timestamp();
                        let texture_to_send = D3D11Texture::new(staging.clone());
                        let frame = CapturedFrame {
                            texture: texture_to_send,
                            bgra,
                            timestamp,
                            resolution: (state.frame_width, state.frame_height),
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
    /// Capture thread entry point
    pub(crate) fn capture_loop(
        running: Arc<AtomicBool>,
        frame_tx: Sender<CapturedFrame>,
        config: CaptureConfig,
    ) {
        info!("DXGI capture thread started: {} FPS", config.target_fps);
        let timeout_ms = (1000u32 / config.target_fps.max(1)).max(1)
            + u32::from(!1000u32.is_multiple_of(config.target_fps.max(1)));
        let frame_interval_ns = 1_000_000_000u64 / config.target_fps.max(1) as u64;
        let mut last_frame_time = std::time::Instant::now();
        let mut frame_count = 0u64;
        let mut error_count = 0u32;
        let max_errors = 10;
        let mut state = match Self::init_capture(config.output_index) {
            Ok(state) => state,
            Err(e) => {
                error!("Failed to initialize DXGI capture: {}", e);
                return;
            }
        };
        info!("DXGI capture initialized and running");
        let mut log_counter = 0u64;
        const LOG_INTERVAL: u64 = 300;
        while running.load(Ordering::Relaxed) {
            match Self::capture_frame(&mut state, timeout_ms) {
                Ok(Some(frame)) => {
                    let elapsed = last_frame_time.elapsed().as_nanos() as u64;
                    if elapsed < frame_interval_ns {
                        let sleep_ns = frame_interval_ns - elapsed;
                        std::thread::sleep(Duration::from_nanos(sleep_ns));
                    }
                    last_frame_time = std::time::Instant::now();
                    match frame_tx.send(frame) {
                        Ok(()) => {
                            frame_count += 1;
                            error_count = 0;
                            if frame_count % LOG_INTERVAL == 0 {
                                log_counter += 1;
                                if log_counter % 10 == 0 {
                                    info!("Captured {} frames", frame_count);
                                } else {
                                    debug!("Captured {} frames", frame_count);
                                }
                            }
                        }
                        Err(crossbeam::channel::SendError(_)) => {
                            info!("Frame channel closed, stopping capture");
                            break;
                        }
                    }
                }
                Ok(None) => {
                    std::thread::yield_now();
                }
                Err(e) => {
                    error!("Capture error: {}", e);
                    error_count += 1;
                    if error_count >= max_errors {
                        error!("Too many capture errors, stopping");
                        break;
                    }
                    warn!("Attempting to reinitialize capture...");
                    match Self::init_capture(config.output_index) {
                        Ok(new_state) => {
                            state = new_state;
                            error_count = 0;
                            info!("Reinitialization successful");
                        }
                        Err(e) => {
                            error!("Reinitialization failed: {}", e);
                            std::thread::sleep(Duration::from_millis(100));
                        }
                    }
                }
            }
        }
        info!(
            "DXGI capture thread stopped ({} frames captured)",
            frame_count
        );
    }
    /// Get current QPC timestamp
    fn get_qpc_timestamp() -> i64 {
        DxgiCaptureState::get_qpc_timestamp()
    }
}
