use std::ffi::c_void;

use anyhow::Result;
use ffmpeg::format::Pixel;
use ffmpeg_next as ffmpeg;
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11DeviceContext, ID3D11Fence,
};

use super::FfmpegEncoder;

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
    pub encoder_device: ID3D11Device,
    pub encoder_fence: Option<ID3D11Fence>,
}

unsafe impl Send for D3d11HardwareContext {}

impl Drop for D3d11HardwareContext {
    fn drop(&mut self) {
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
    pub(super) fn create_hw_frames_ctx_with_pool_size(
        device_ctx_ref: *mut ffmpeg::ffi::AVBufferRef,
        width: u32,
        height: u32,
        initial_pool_size: i32,
    ) -> Result<*mut ffmpeg::ffi::AVBufferRef> {
        unsafe {
            let frames_ctx_ref = ffmpeg::ffi::av_hwframe_ctx_alloc(device_ctx_ref);
            if frames_ctx_ref.is_null() {
                anyhow::bail!("Failed to allocate FFmpeg D3D11 frame context");
            }

            let frames_ctx = (*frames_ctx_ref).data as *mut ffmpeg::ffi::AVHWFramesContext;
            (*frames_ctx).format = Pixel::D3D11.into();
            (*frames_ctx).sw_format = Pixel::NV12.into();
            (*frames_ctx).width = width as i32;
            (*frames_ctx).height = height as i32;
            (*frames_ctx).initial_pool_size = initial_pool_size;

            let init_frames_result = ffmpeg::ffi::av_hwframe_ctx_init(frames_ctx_ref);
            if init_frames_result < 0 {
                let mut frames_ctx_ref = frames_ctx_ref;
                ffmpeg::ffi::av_buffer_unref(&mut frames_ctx_ref);
                anyhow::bail!(
                    "Failed to initialize FFmpeg D3D11 frame context with pool size {}: {}",
                    initial_pool_size,
                    init_frames_result
                );
            }

            Ok(frames_ctx_ref)
        }
    }
}
