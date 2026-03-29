//! DXGI desktop duplication capture.

use crate::capture::{
    backpressure::BackpressureState, CaptureConfig, CapturedFrame, D3d11TexturePoolItem,
};
use anyhow::{bail, Context, Result};
use bytes::Bytes;
use crossbeam::channel::{bounded, Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};
use windows::Win32::Foundation::{CloseHandle, BOOL, HANDLE};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11Device5, ID3D11DeviceContext, ID3D11Fence, ID3D11Multithread,
    ID3D11Texture2D, ID3D11VideoContext, ID3D11VideoDevice, ID3D11VideoProcessor,
    ID3D11VideoProcessorEnumerator, D3D11_FENCE_FLAG_SHARED, D3D11_VIDEO_PROCESSOR_COLOR_SPACE,
    D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_RATIONAL,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIOutput1, IDXGIResource, DXGI_ERROR_ACCESS_DENIED,
    DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_INVALID_CALL, DXGI_ERROR_NON_COMPOSITED_UI,
    DXGI_ERROR_NOT_CURRENTLY_AVAILABLE, DXGI_ERROR_UNSUPPORTED, DXGI_ERROR_WAIT_TIMEOUT,
    DXGI_OUTDUPL_FRAME_INFO,
};
use windows::Win32::System::Performance::QueryPerformanceCounter;
use windows_core::Interface;

pub(super) struct Nv12TexturePool {
    pub(super) available: Vec<D3d11TexturePoolItem>,
    pub(super) return_tx: Sender<D3d11TexturePoolItem>,
    pub(super) return_rx: Receiver<D3d11TexturePoolItem>,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) max_capacity: usize,
    pub(super) total_created: std::sync::atomic::AtomicUsize,
}

pub(super) struct BgraTexturePool {
    pub(super) available: Vec<D3d11TexturePoolItem>,
    pub(super) return_tx: Sender<D3d11TexturePoolItem>,
    pub(super) return_rx: Receiver<D3d11TexturePoolItem>,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) max_capacity: usize,
    pub(super) total_created: std::sync::atomic::AtomicUsize,
}

enum CaptureOutcome {
    Frame(CapturedFrame),
    Timeout,
    Dropped,
}

impl Nv12TexturePool {
    fn new(width: u32, height: u32) -> Self {
        let max_capacity = 12usize;
        let (return_tx, return_rx) = bounded(max_capacity * 2);
        Self {
            available: Vec::new(),
            return_tx,
            return_rx,
            width,
            height,
            max_capacity,
            total_created: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

impl BgraTexturePool {
    fn new(width: u32, height: u32) -> Self {
        let max_capacity = 12usize;
        let (return_tx, return_rx) = bounded(max_capacity * 2);
        Self {
            available: Vec::new(),
            return_tx,
            return_rx,
            width,
            height,
            max_capacity,
            total_created: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

/// Low-power mode FPS for adaptive capture rate when scene is static
const LOW_POWER_FPS: u32 = 5;

/// DXGI capture state (desktop resolution capture; resize to configured output is done in the encoder).
pub(super) struct DxgiCaptureState {
    pub(super) d3d_device: ID3D11Device,
    pub(super) d3d_context: ID3D11DeviceContext,
    pub(super) duplication: windows::Win32::Graphics::Dxgi::IDXGIOutputDuplication,
    pub(super) frame_width: u32,
    pub(super) frame_height: u32,
    pub(super) bgra_pool: Option<BgraTexturePool>,
    pub(super) nv12_pool: Option<Nv12TexturePool>,
    pub(super) video_device: Option<ID3D11VideoDevice>,
    pub(super) video_context: Option<ID3D11VideoContext>,
    pub(super) video_processor: Option<ID3D11VideoProcessor>,
    pub(super) video_processor_enumerator: Option<ID3D11VideoProcessorEnumerator>,
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

    /// Initialize DXGI capture state (desktop resolution; encoder applies configured output size).
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

            // Initialize NV12 conversion resources for hardware encoding
            let (
                nv12_pool,
                video_device,
                video_context,
                video_processor,
                video_processor_enumerator,
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
                    (None, None, None, None, None, false)
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

            // Staging / CPU readback uses full desktop resolution; encoder scales to target.
            if needs_scaling {
                info!(
                    "DXGI capture initialized (encoder-side scaling): {}x{} -> {}x{}",
                    frame_width, frame_height, target_width, target_height
                );
            } else {
                info!("DXGI capture initialized: {}x{}", frame_width, frame_height);
            }

            Ok(DxgiCaptureState {
                d3d_device,
                d3d_context,
                duplication,
                frame_width,
                frame_height,
                bgra_pool: Some(bgra_pool),
                nv12_pool,
                video_device,
                video_context,
                video_processor,
                video_processor_enumerator,
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
                true,
            ))
        }
    }
}

impl Drop for DxgiCaptureState {
    fn drop(&mut self) {
        // Close the NT kernel handles that back the shared D3D11 fences.
        // `ID3D11Fence` COM objects are released automatically by their Drop impls, but the
        // *shared* handle returned by `CreateSharedHandle` is a separate NT object that must
        // be closed explicitly with `CloseHandle`.
        unsafe {
            if let Some(h) = self.nv12_fence_shared_handle.take() {
                if !h.is_invalid() {
                    let _ = CloseHandle(h);
                }
            }
            if let Some(h) = self.bgra_fence_shared_handle.take() {
                if !h.is_invalid() {
                    let _ = CloseHandle(h);
                }
            }
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
/// - D3D11 Video Processor BGRA→NV12 for hardware encoding when available
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
    /// use liteclip_core::capture::dxgi::DxgiCapture;
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

    /// Capture a single frame (GPU-only path)
    fn capture_frame(
        state: &mut DxgiCaptureState,
        timeout_ms: u32,
        drop_before_process: bool,
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

                    // GPU-only capture path - always use GPU frames
                    let frame = DxgiCapture::capture_gpu_frame(
                        state,
                        &captured_texture,
                        timestamp,
                        gpu_texture_format,
                    )?;
                    state.duplication.ReleaseFrame().ok();
                    return Ok(CaptureOutcome::Frame(frame));
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
    /// 3. Handles frame capture, CPU readback (if configured), and NV12 conversion when available.
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

        info!(
            "DXGI capture thread started: {} FPS (GPU-only mode)",
            config.target_fps
        );
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
        // If a non-native output size is configured, still capture at desktop resolution; encoder scales.
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
        // No final spin: sleep only, to reduce CPU (slightly looser pacing).
        const FRAME_PACING_SPIN_WINDOW: Duration = Duration::from_micros(0);
        let mut frame_period = Duration::from_nanos(1_000_000_000u64 / base_fps as u64);
        let mut next_frame_time = std::time::Instant::now();

        // Adaptive capture rate: scene change detection
        let mut last_present_qpc: i64 = 0;
        let _consecutive_static_frames: u32 = 0;
        let low_power_frame_period = Duration::from_nanos(1_000_000_000u64 / LOW_POWER_FPS as u64);
        let mut is_low_power_mode = false;

        while running.load(Ordering::Relaxed) {
            let mut drop_before_process = false;
            let queue_len = frame_tx.len() as u32;
            let queue_cap = frame_tx.capacity().unwrap_or(32) as u32;
            let fps_divisor = backpressure.current_fps_divisor();
            if fps_divisor > 0 {
                adaptive_skip_counter = adaptive_skip_counter.wrapping_add(1);
                drop_before_process = adaptive_skip_counter % (fps_divisor + 1) != 0;
            }
            if !drop_before_process && queue_len.saturating_add(1) >= queue_cap {
                drop_before_process = true;
            }

            // Check for scene changes when in low-power mode
            let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
            if is_low_power_mode {
                // Peek at frame info without acquiring
                let hr = unsafe {
                    state.duplication.AcquireNextFrame(
                        1, // 1ms timeout for quick check
                        &mut frame_info,
                        &mut None,
                    )
                };

                if hr.is_ok() {
                    let current_present_qpc = frame_info.LastPresentTime;
                    let is_new_content =
                        current_present_qpc != last_present_qpc && current_present_qpc != 0;

                    unsafe { state.duplication.ReleaseFrame().ok() };

                    if is_new_content {
                        // Scene changed, exit low-power mode
                        is_low_power_mode = false;
                        last_present_qpc = current_present_qpc;
                        frame_period = Duration::from_nanos(1_000_000_000u64 / base_fps as u64);
                        info!(
                            "Adaptive capture: resumed full-rate capture (scene change detected)"
                        );
                    } else {
                        // Still static, continue low-power mode
                        // Send duplicate frame to maintain timing
                        if let Some(ref last) = last_frame {
                            if backpressure.is_encoder_overloaded() || frame_tx.is_full() {
                                // Encoder busy, skip this frame
                            } else {
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
                                if frame_tx.try_send(frame).is_ok() {
                                    duplicated_count += 1;
                                    window_duplicates += 1;
                                    frame_count += 1;
                                    window_frames += 1;
                                }
                            }
                        }

                        // Sleep for low-power interval
                        std::thread::sleep(low_power_frame_period);
                        next_frame_time = std::time::Instant::now() + low_power_frame_period;
                        continue;
                    }
                }
            }

            match Self::capture_frame(
                &mut state,
                timeout_ms,
                drop_before_process,
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
                        if adaptive_skip_counter % (fps_divisor + 1) != 0 {
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
                let nv12_stats = state.nv12_pool.as_ref().map(|p| {
                    (
                        p.available.len(),
                        p.total_created.load(std::sync::atomic::Ordering::Relaxed),
                        p.max_capacity,
                    )
                });
                let bgra_stats = state.bgra_pool.as_ref().map(|p| {
                    (
                        p.available.len(),
                        p.total_created.load(std::sync::atomic::Ordering::Relaxed),
                        p.max_capacity,
                    )
                });
                if let Some((working_set_mb, private_mb)) =
                    crate::output::saver::process_memory_mb()
                {
                    info!(
                        "Memory telemetry [capture]: fps={}, drops={}, duplicates={}, divisor={}, queue={}/{}, nv12={:?}, bgra={:?}, process_working_set_mb={:.1}, process_private_mb={:.1}",
                        window_frames / 30,
                        window_drops,
                        window_duplicates,
                        backpressure.current_fps_divisor(),
                        frame_tx.len(),
                        frame_tx.capacity().unwrap_or(32),
                        nv12_stats,
                        bgra_stats,
                        working_set_mb,
                        private_mb
                    );
                } else {
                    info!(
                        "Memory telemetry [capture]: fps={}, drops={}, duplicates={}, divisor={}, queue={}/{}, nv12={:?}, bgra={:?}",
                        window_frames / 30,
                        window_drops,
                        window_duplicates,
                        backpressure.current_fps_divisor(),
                        frame_tx.len(),
                        frame_tx.capacity().unwrap_or(32),
                        nv12_stats,
                        bgra_stats
                    );
                }
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
            }
            next_frame_time += frame_period;
            // If we're significantly behind, reset the schedule
            if std::time::Instant::now() > next_frame_time + frame_period {
                next_frame_time = std::time::Instant::now() + frame_period;
            }
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
