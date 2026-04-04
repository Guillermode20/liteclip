use std::ffi::c_void;

use ffmpeg::format::Pixel;
use ffmpeg_next as ffmpeg;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11DeviceContext, ID3D11Fence, ID3D11Texture2D,
};
use windows_core::Interface;

use crate::encode::{EncodeError, EncodeResult};

use super::FfmpegEncoder;

const MAX_CACHED_SHARED_TEXTURES: usize = 32;

#[repr(C)]
pub(super) struct AvD3d11vaDeviceContext {
    pub device: *mut c_void,
    pub device_context: *mut c_void,
    pub video_device: *mut c_void,
    pub video_context: *mut c_void,
    pub lock: Option<unsafe extern "C" fn(*mut c_void)>,
    pub unlock: Option<unsafe extern "C" fn(*mut c_void)>,
    pub lock_ctx: *mut c_void,
}

pub(super) struct D3d11HardwareContext {
    pub device_ctx_ref: *mut ffmpeg::ffi::AVBufferRef,
    pub frames_ctx_ref: *mut ffmpeg::ffi::AVBufferRef,
    pub copy_context: ID3D11DeviceContext,
    pub reusable_hw_frame: *mut ffmpeg::ffi::AVFrame,
    pub encoder_device: Option<ID3D11Device>,
    pub encoder_fence: Option<ID3D11Fence>,
    pub cached_shared_textures: Vec<(HANDLE, ID3D11Texture2D)>,
    pub is_shared_device: bool,
}

// SAFETY: D3d11HardwareContext is Send because:
// 1. The raw pointers (device_ctx_ref, frames_ctx_ref, reusable_hw_frame) are FFmpeg
//    resources that are only accessed from the encoder thread
// 2. ID3D11Device, ID3D11DeviceContext, and ID3D11Fence are COM interfaces that are
//    thread-safe when used correctly (single-threaded access in our case)
// 3. The Drop implementation properly cleans up FFmpeg resources via av_buffer_unref
// 4. cached_shared_textures contains COM references that are properly managed
// 5. The context is created on and only used from the encoder thread
unsafe impl Send for D3d11HardwareContext {}

impl Drop for D3d11HardwareContext {
    fn drop(&mut self) {
        // Drop cached shared textures first to release their COM references
        // (the shared HANDLEs are owned by the capture/pool and should not be closed here).
        self.cached_shared_textures.clear();

        unsafe {
            if !self.reusable_hw_frame.is_null() {
                ffmpeg::ffi::av_frame_free(&mut self.reusable_hw_frame);
            }
            if !self.frames_ctx_ref.is_null() {
                ffmpeg::ffi::av_buffer_unref(&mut self.frames_ctx_ref);
            }
            if !self.device_ctx_ref.is_null() {
                ffmpeg::ffi::av_buffer_unref(&mut self.device_ctx_ref);
            }
        }
    }
}

impl FfmpegEncoder {
    pub(super) fn create_d3d11_hardware_context_from_device(
        &self,
        device: &ID3D11Device,
        width: u32,
        height: u32,
    ) -> EncodeResult<D3d11HardwareContext> {
        unsafe {
            let mut device_ctx_ref = ffmpeg::ffi::av_hwdevice_ctx_alloc(
                ffmpeg::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA,
            );
            if device_ctx_ref.is_null() {
                return Err(EncodeError::msg(
                    "Failed to allocate FFmpeg D3D11 device context",
                ));
            }

            let hw_device_ctx = (*device_ctx_ref).data as *mut ffmpeg::ffi::AVHWDeviceContext;
            let d3d11_ctx = (*hw_device_ctx).hwctx as *mut AvD3d11vaDeviceContext;

            let context = device
                .GetImmediateContext()
                .map_err(|e| EncodeError::msg(format!("Failed to get immediate context: {}", e)))?;

            let ffmpeg_device = device.clone();
            let ffmpeg_context = context.clone();
            (*d3d11_ctx).device = ffmpeg_device.as_raw() as *mut _;
            (*d3d11_ctx).device_context = ffmpeg_context.as_raw() as *mut _;

            // Store the objects in the struct to keep them alive
            let device_for_storage = ffmpeg_device;
            let context_for_storage = context;

            let init_result = ffmpeg::ffi::av_hwdevice_ctx_init(device_ctx_ref);
            if init_result < 0 {
                ffmpeg::ffi::av_buffer_unref(&mut device_ctx_ref);
                return Err(EncodeError::msg(format!(
                    "Failed to initialize FFmpeg D3D11 hardware device context: {}",
                    init_result
                )));
            }

            let mut frames_ctx_ref = Self::create_hw_frames_ctx_with_pool_size(
                device_ctx_ref,
                width,
                height,
                Pixel::NV12,
                0, // 0 = dynamic pool, let us provide textures
            )?;

            let reusable_hw_frame = ffmpeg::ffi::av_frame_alloc();
            if reusable_hw_frame.is_null() {
                ffmpeg::ffi::av_buffer_unref(&mut frames_ctx_ref);
                ffmpeg::ffi::av_buffer_unref(&mut device_ctx_ref);
                return Err(EncodeError::msg(
                    "Failed to allocate reusable hardware frame",
                ));
            }

            Ok(D3d11HardwareContext {
                device_ctx_ref,
                frames_ctx_ref,
                copy_context: context_for_storage,
                reusable_hw_frame,
                encoder_device: Some(device_for_storage),
                encoder_fence: None,
                cached_shared_textures: Vec::new(),
                is_shared_device: true,
            })
        }
    }

    pub(super) fn create_hw_frames_ctx_with_pool_size(
        device_ctx_ref: *mut ffmpeg::ffi::AVBufferRef,
        width: u32,
        height: u32,
        sw_format: Pixel,
        initial_pool_size: i32,
    ) -> EncodeResult<*mut ffmpeg::ffi::AVBufferRef> {
        unsafe {
            let frames_ctx_ref = ffmpeg::ffi::av_hwframe_ctx_alloc(device_ctx_ref);
            if frames_ctx_ref.is_null() {
                return Err(EncodeError::msg(
                    "Failed to allocate FFmpeg D3D11 frame context",
                ));
            }

            let frames_ctx = (*frames_ctx_ref).data as *mut ffmpeg::ffi::AVHWFramesContext;
            (*frames_ctx).format = Pixel::D3D11.into();
            (*frames_ctx).sw_format = sw_format.into();
            (*frames_ctx).width = width as i32;
            (*frames_ctx).height = height as i32;
            (*frames_ctx).initial_pool_size = initial_pool_size;

            let init_frames_result = ffmpeg::ffi::av_hwframe_ctx_init(frames_ctx_ref);
            if init_frames_result < 0 {
                let mut frames_ctx_ref = frames_ctx_ref;
                ffmpeg::ffi::av_buffer_unref(&mut frames_ctx_ref);
                return Err(EncodeError::msg(format!(
                    "Failed to initialize FFmpeg D3D11 frame context with pool size {}: {}",
                    initial_pool_size, init_frames_result
                )));
            }

            Ok(frames_ctx_ref)
        }
    }

    pub(super) unsafe fn prepare_hw_frame(
        hw_context: &mut D3d11HardwareContext,
        hw_frame: *mut ffmpeg::ffi::AVFrame,
        gpu_frame: &crate::media::D3d11Frame,
    ) -> EncodeResult<()> {
        ffmpeg::ffi::av_frame_unref(hw_frame);

        let get_buffer_res =
            ffmpeg::ffi::av_hwframe_get_buffer(hw_context.frames_ctx_ref, hw_frame, 0);
        if get_buffer_res < 0 {
            return Err(EncodeError::msg(format!(
                "Failed to get hardware frame buffer: {}",
                get_buffer_res
            )));
        }

        // Validate that FFmpeg properly allocated the hardware frame
        if hw_frame.is_null() {
            return Err(EncodeError::msg(
                "av_hwframe_get_buffer returned null frame pointer",
            ));
        }

        // For shared device, use the source texture directly
        (*hw_frame).data[0] = gpu_frame.texture().unwrap().as_raw() as *mut _;
        (*hw_frame).data[1] = std::ptr::null_mut();
        (*hw_frame).data[2] = std::ptr::null_mut();
        (*hw_frame).data[3] = std::ptr::null_mut();

        if !hw_context.is_shared_device {
            // Cross-device logic (OpenSharedResource + Fence wait)
            // SAFETY: hw_frame validated above, but data[0] could still be null if FFmpeg
            // returned success but didn't properly allocate frame data
            if (*hw_frame).data[0].is_null() {
                return Err(EncodeError::msg(
                    "av_hwframe_get_buffer succeeded but data[0] is null",
                ));
            }
            let texture_ptr = (*hw_frame).data[0] as *mut ID3D11Texture2D;
            let dst_texture = &*texture_ptr;
            let dest_subresource = (*hw_frame).data[1] as usize as u32;

            let shared_handle = gpu_frame.shared_handle().unwrap();
            let shared_texture = if let Some(found) = hw_context
                .cached_shared_textures
                .iter()
                .find(|(h, _)| *h == shared_handle)
                .map(|(_, t)| t)
            {
                found.clone()
            } else {
                let mut opened_opt: Option<ID3D11Texture2D> = None;
                if let Some(ref encoder_device) = hw_context.encoder_device {
                    encoder_device
                        .OpenSharedResource(shared_handle, &mut opened_opt)
                        .map_err(|e| {
                            EncodeError::msg(format!(
                                "Failed to open shared texture on encoder device: {}",
                                e
                            ))
                        })?;
                } else {
                    return Err(EncodeError::msg(
                        "No encoder device available for cross-device texture sharing",
                    ));
                }
                let opened = opened_opt
                    .ok_or_else(|| EncodeError::msg("OpenSharedResource returned null"))?;
                hw_context
                    .cached_shared_textures
                    .push((shared_handle, opened.clone()));
                if hw_context.cached_shared_textures.len() > MAX_CACHED_SHARED_TEXTURES {
                    hw_context.cached_shared_textures.remove(0);
                }
                opened
            };

            if let (Some(ref fence), Some(_handle)) =
                (&hw_context.encoder_fence, gpu_frame.fence_shared_handle)
            {
                let ctx4: windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext4 =
                    hw_context.copy_context.cast().map_err(|e| {
                        EncodeError::msg(format!(
                            "Failed to get ID3D11DeviceContext4 for encoder wait: {}",
                            e
                        ))
                    })?;
                ctx4.Wait(fence, gpu_frame.fence_value).map_err(|e| {
                    EncodeError::msg(format!("Failed to wait on shared fence in encoder: {}", e))
                })?;
            }

            hw_context.copy_context.CopySubresourceRegion(
                dst_texture,
                dest_subresource,
                0,
                0,
                0,
                &shared_texture,
                0,
                None,
            );
        }

        Ok(())
    }
}
