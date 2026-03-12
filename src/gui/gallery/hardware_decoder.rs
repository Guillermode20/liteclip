use anyhow::{bail, Context, Result};
use std::ffi::CString;
use std::ptr;
use std::sync::Arc;

use ffmpeg_next as ffmpeg;
use tracing::{info, warn};

#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_1;
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_SDK_VERSION,
};
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Dxgi::IDXGIAdapter;
#[cfg(target_os = "windows")]
use windows_core::Interface;

pub struct HardwareDecoder {
    decoder_context: ffmpeg::codec::context::Context,
    hw_device_ctx: *mut ffmpeg::ffi::AVBufferRef,
    hw_frames_ctx: *mut ffmpeg::ffi::AVBufferRef,
    width: u32,
    height: u32,
    #[cfg(target_os = "windows")]
    d3d11_device: Option<ID3D11Device>,
}

#[cfg(target_os = "windows")]
pub struct D3D11DecodeDevice {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
}

#[cfg(target_os = "windows")]
impl D3D11DecodeDevice {
    pub fn new() -> Result<Self> {
        unsafe {
            let feature_levels = [D3D_FEATURE_LEVEL_11_1];
            let mut device_opt: Option<ID3D11Device> = None;
            let mut context_opt: Option<ID3D11DeviceContext> = None;

            D3D11CreateDevice(
                None,
                windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE,
                windows::Win32::Foundation::HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&feature_levels),
                D3D11_SDK_VERSION,
                Some(&mut device_opt),
                None,
                Some(&mut context_opt),
            )
            .ok()
            .context("Failed to create D3D11 device for hardware decode")?;

            let device = device_opt.context("D3D11 device is null")?;
            let context = context_opt.context("D3D11 context is null")?;

            Ok(Self { device, context })
        }
    }

    pub fn from_existing(device: ID3D11Device) -> Result<Self> {
        unsafe {
            let mut context_opt: Option<ID3D11DeviceContext> = None;
            device.GetImmediateContext(&mut context_opt);
            let context = context_opt.context("Failed to get immediate context")?;
            Ok(Self { device, context })
        }
    }
}

impl HardwareDecoder {
    pub fn new(
        video_path: &std::path::Path,
        output_width: u32,
        output_height: u32,
    ) -> Result<Self> {
        let format = ffmpeg::format::input(&video_path)
            .with_context(|| format!("Failed to open video file: {:?}", video_path))?;

        let stream = format
            .streams()
            .best(ffmpeg::media::Type::Video)
            .context("No video stream found")?;

        let stream_index = stream.index();
        let context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
        let codec = ffmpeg::decoder::find(context.codec_id())
            .with_context(|| format!("Decoder not found for {:?}", context.codec_id()))?;

        let input_width = context.width();
        let input_height = context.height();

        let hw_device_ctx = Self::create_hw_device_ctx()?;
        let hw_frames_ctx = Self::create_hw_frames_ctx(hw_device_ctx, input_width, input_height)?;

        let mut decoder = context.decoder().video()?;
        unsafe {
            (*decoder.as_mut_ptr()).hw_device_ctx = ffmpeg::ffi::av_buffer_ref(hw_device_ctx);
            (*decoder.as_mut_ptr()).hw_frames_ctx = ffmpeg::ffi::av_buffer_ref(hw_frames_ctx);
        }

        let decoder = decoder.open_with(ffmpeg::Dictionary::new())?;

        let decoder_context = ffmpeg::codec::context::Context::new_with_codec(codec);

        Ok(Self {
            decoder_context: decoder,
            hw_device_ctx,
            hw_frames_ctx,
            width: output_width,
            height: output_height,
            #[cfg(target_os = "windows")]
            d3d11_device: None,
        })
    }

    #[cfg(target_os = "windows")]
    pub fn with_d3d11_device(
        video_path: &std::path::Path,
        output_width: u32,
        output_height: u32,
        d3d11_device: ID3D11Device,
    ) -> Result<Self> {
        let mut decoder = Self::new(video_path, output_width, output_height)?;
        decoder.d3d11_device = Some(d3d11_device);
        Ok(decoder)
    }

    unsafe fn create_hw_device_ctx() -> Result<*mut ffmpeg::ffi::AVBufferRef> {
        let device_type_name = CString::new("d3d11va").expect("static string");
        let device_type = ffmpeg::ffi::av_hwdevice_find_type_by_name(device_type_name.as_ptr());

        if device_type == ffmpeg::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_NONE {
            bail!("D3D11VA hardware device type not available");
        }

        let device_ctx_ref = ffmpeg::ffi::av_hwdevice_ctx_alloc(device_type);
        if device_ctx_ref.is_null() {
            bail!("Failed to allocate hardware device context");
        }

        let result = ffmpeg::ffi::av_hwdevice_ctx_init(device_ctx_ref);
        if result < 0 {
            ffmpeg::ffi::av_buffer_unref(&mut device_ctx_ref as *mut _);
            bail!("Failed to initialize hardware device context: {}", result);
        }

        Ok(device_ctx_ref)
    }

    unsafe fn create_hw_frames_ctx(
        device_ctx: *mut ffmpeg::ffi::AVBufferRef,
        width: u32,
        height: u32,
    ) -> Result<*mut ffmpeg::ffi::AVBufferRef> {
        let frames_ctx_ref = ffmpeg::ffi::av_hwframe_ctx_alloc(device_ctx);
        if frames_ctx_ref.is_null() {
            bail!("Failed to allocate hardware frames context");
        }

        let hw_frames_ctx = (*frames_ctx_ref).data as *mut ffmpeg::ffi::AVHWFramesContext;
        (*hw_frames_ctx).format = ffmpeg::ffi::AVPixelFormat::AV_PIX_FMT_D3D11;
        (*hw_frames_ctx).sw_format = ffmpeg::ffi::AVPixelFormat::AV_PIX_FMT_NV12;
        (*hw_frames_ctx).width = width as i32;
        (*hw_frames_ctx).height = height as i32;
        (*hw_frames_ctx).initial_pool_size = 4;

        let result = ffmpeg::ffi::av_hwframe_ctx_init(frames_ctx_ref);
        if result < 0 {
            ffmpeg::ffi::av_buffer_unref(&mut frames_ctx_ref as *mut _);
            bail!("Failed to initialize hardware frames context: {}", result);
        }

        Ok(frames_ctx_ref)
    }

    pub fn decode_frame_at(&mut self, _time_secs: f64) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }

    pub fn decode_frame(&mut self, _packet: &ffmpeg::Packet) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }
}

impl Drop for HardwareDecoder {
    fn drop(&mut self) {
        unsafe {
            if !self.hw_frames_ctx.is_null() {
                ffmpeg::ffi::av_buffer_unref(&mut self.hw_frames_ctx);
            }
            if !self.hw_device_ctx.is_null() {
                ffmpeg::ffi::av_buffer_unref(&mut self.hw_device_ctx);
            }
        }
    }
}

pub fn is_hardware_decode_available() -> bool {
    unsafe {
        let device_type_name = CString::new("d3d11va").expect("static string");
        let device_type = ffmpeg::ffi::av_hwdevice_find_type_by_name(device_type_name.as_ptr());
        device_type != ffmpeg::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_NONE
    }
}

pub fn find_decoder_for_hwaccel(
    codec_id: ffmpeg::ffi::AVCodecID,
) -> Option<ffmpeg::decoder::Decoder> {
    let codec = ffmpeg::decoder::find(codec_id)?;

    unsafe {
        let codec_ptr = codec.as_ptr();
        let mut iter: *mut ffmpeg::ffi::AVCodecHWConfig = ptr::null_mut();
        let mut i = 0;

        loop {
            iter = ffmpeg::ffi::avcodec_get_hw_config(codec_ptr, i);
            if iter.is_null() {
                break;
            }

            if (*iter).pix_fmt == ffmpeg::ffi::AVPixelFormat::AV_PIX_FMT_D3D11 {
                return Some(codec);
            }
            i += 1;
        }
    }

    Some(codec)
}
