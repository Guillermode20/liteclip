//! Auto-generated module
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use crate::capture::{backpressure::BackpressureState, CaptureConfig, CapturedFrame};
use anyhow::{bail, Context, Result};
use bytes::{Bytes, BytesMut};
use crossbeam::channel::{bounded, Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
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
use windows_core::Interface;

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
        // 4 frames at 2560x1440 BGRA = ~60MB (vs 32 frames = ~470MB)
        let (frame_tx, frame_rx) = bounded::<CapturedFrame>(4);
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
            let native_size = (frame_width * frame_height * 4) as usize;
            let mut native_buffer = BytesMut::with_capacity(native_size);
            native_buffer.resize(native_size, 0);
            Ok(DxgiCaptureState {
                d3d_device,
                d3d_context,
                duplication,
                output_desc,
                staging_texture: None,
                frame_width,
                frame_height,
                native_buffer,
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
        perform_cpu_readback: bool,
        target_resolution: Option<(u32, u32)>,
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
                    let native_res = (state.frame_width, state.frame_height);
                    if !perform_cpu_readback {
                        state.duplication.ReleaseFrame().ok();
                        let output_res = target_resolution.unwrap_or(native_res);
                        let frame = CapturedFrame {
                            bgra: Bytes::new(),
                            timestamp,
                            resolution: output_res,
                        };
                        return Ok(Some(frame));
                    }
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
                        let src_w = state.frame_width as usize;
                        let src_h = state.frame_height as usize;
                        let src_row_bytes = src_w * 4;
                        let src_pitch = mapped.RowPitch as usize;
                        if mapped.pData.is_null() {
                            state.d3d_context.Unmap(Some(&staging_resource), 0);
                            bail!("Mapped staging texture has null data pointer");
                        }
                        let total_src_bytes = (src_h.saturating_sub(1))
                            .checked_mul(src_pitch)
                            .and_then(|v| v.checked_add(src_row_bytes));
                        if total_src_bytes.is_none()
                            || total_src_bytes.unwrap() > isize::MAX as usize
                        {
                            state.d3d_context.Unmap(Some(&staging_resource), 0);
                            bail!(
                                "Frame dimensions too large for safe copy: {}x{}, pitch={}",
                                src_w,
                                src_h,
                                src_pitch
                            );
                        }
                        let src_ptr = mapped.pData as *const u8;
                        let total_bytes = src_row_bytes * src_h;
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
                            for _row in 0..src_h {
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
                            timestamp,
                            resolution: (src_w as u32, src_h as u32),
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
        overflow_rx: Receiver<CapturedFrame>,
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
        let mut _window_dropped_oldest = 0u64;
        let mut error_count = 0u32;
        let max_errors = 10;
        let mut reinit_backoff_ms = 100u64;
        const MAX_BACKOFF_MS: u64 = 5000;
        let backpressure = BackpressureState::new();
        let mut adaptive_skip_counter = 0u32;
        let mut adaptive_adjust_tick = Instant::now();
        let mut _adaptive_level_changes = 0u64;
        let mut state = match Self::init_capture(config.output_index) {
            Ok(state) => state,
            Err(e) => {
                error!("Failed to initialize DXGI capture: {}", e);
                let _ = fatal_tx.try_send(format!("Failed to initialize DXGI capture: {}", e));
                return;
            }
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
                        Err(crossbeam::channel::TrySendError::Full(frame)) => {
                            dropped_count += 1;
                            window_drops += 1;
                            error_count = 0;
                            backpressure.set_encoder_overloaded(true);
                            let mut dropped_oldest = false;
                            if overflow_rx.try_recv().is_ok() {
                                dropped_oldest = true;
                                _window_dropped_oldest += 1;
                                match frame_tx.try_send(frame) {
                                    Ok(()) => {
                                        frame_count += 1;
                                        window_frames += 1;
                                    }
                                    Err(crossbeam::channel::TrySendError::Full(_)) => {
                                        dropped_count += 1;
                                        window_drops += 1;
                                    }
                                    Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
                                        info!("Frame channel closed, stopping capture");
                                        break;
                                    }
                                }
                            }
                            if dropped_count % 60 == 0 {
                                warn!(
                                    "Dropped {} frames (encoder behind, drop_oldest={})",
                                    dropped_count, dropped_oldest
                                );
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
                        Err(crossbeam::channel::TrySendError::Full(frame)) => {
                            dropped_count += 1;
                            window_drops += 1;
                            error_count = 0;
                            backpressure.set_encoder_overloaded(true);
                            let mut dropped_oldest = false;
                            if overflow_rx.try_recv().is_ok() {
                                dropped_oldest = true;
                                _window_dropped_oldest += 1;
                                match frame_tx.try_send(frame) {
                                    Ok(()) => {
                                        frame_count += 1;
                                        window_frames += 1;
                                    }
                                    Err(crossbeam::channel::TrySendError::Full(_)) => {
                                        dropped_count += 1;
                                        window_drops += 1;
                                    }
                                    Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
                                        info!("Frame channel closed, stopping capture");
                                        break;
                                    }
                                }
                            }
                            if dropped_count % 60 == 0 {
                                warn!(
                                    "Dropped {} frames (encoder behind, drop_oldest={})",
                                    dropped_count, dropped_oldest
                                );
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
                    match Self::init_capture(config.output_index) {
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
