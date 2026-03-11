use std::ffi::c_void;

use anyhow::{Context, Result};
use ffmpeg::format::Pixel;
use ffmpeg_next as ffmpeg;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11DeviceContext, ID3D11Fence, ID3D11Texture2D,
};
use windows_core::Interface;

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
    pub encoder_device: Option<ID3D11Device>,
    pub encoder_fence: Option<ID3D11Fence>,
    pub cached_shared_textures: Vec<(HANDLE, ID3D11Texture2D)>,
    pub is_shared_device: bool,
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
    pub(super) fn create_d3d11_hardware_context_from_device(
        &self,
        device: &ID3D11Device,
        width: u32,
        height: u32,
    ) -> Result<D3d11HardwareContext> {
        unsafe {
            let mut device_ctx_ref = ffmpeg::ffi::av_hwdevice_ctx_alloc(
                ffmpeg::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA,
            );
            if device_ctx_ref.is_null() {
                anyhow::bail!("Failed to allocate FFmpeg D3D11 device context");
            }

            let hw_device_ctx = (*device_ctx_ref).data as *mut ffmpeg::ffi::AVHWDeviceContext;
            let d3d11_ctx = (*hw_device_ctx).hwctx as *mut AvD3d11vaDeviceContext;

            let context = device
                .GetImmediateContext()
                .context("Failed to get immediate context")?;

            let ffmpeg_device = device.clone();
            let ffmpeg_context = context.clone();
            (*d3d11_ctx).device = ffmpeg_device.as_raw() as *mut _;
            (*d3d11_ctx).device_context = ffmpeg_context.as_raw() as *mut _;

            // Store the objects in the struct to keep them alive
            let device_for_storage = ffmpeg_device.clone();
            let context_for_storage = context.clone();

            let init_result = ffmpeg::ffi::av_hwdevice_ctx_init(device_ctx_ref);
            if init_result < 0 {
                ffmpeg::ffi::av_buffer_unref(&mut device_ctx_ref);
                anyhow::bail!(
                    "Failed to initialize FFmpeg D3D11 hardware device context: {}",
                    init_result
                );
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
                ffmpeg::ffi::av_buffer_unref(&mut frames_ctx_ref as *mut _);
                ffmpeg::ffi::av_buffer_unref(&mut device_ctx_ref);
                anyhow::bail!("Failed to allocate reusable hardware frame");
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
    ) -> Result<*mut ffmpeg::ffi::AVBufferRef> {
        unsafe {
            let frames_ctx_ref = ffmpeg::ffi::av_hwframe_ctx_alloc(device_ctx_ref);
            if frames_ctx_ref.is_null() {
                anyhow::bail!("Failed to allocate FFmpeg D3D11 frame context");
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
