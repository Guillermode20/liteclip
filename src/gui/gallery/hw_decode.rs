use anyhow::{bail, Context, Result};
use ffmpeg_next as ffmpeg;
use std::ffi::CString;
use std::ptr;

pub struct HwDeviceContext {
    ctx: *mut ffmpeg::ffi::AVBufferRef,
    device_type: ffmpeg::ffi::AVHWDeviceType,
}

unsafe impl Send for HwDeviceContext {}
unsafe impl Sync for HwDeviceContext {}

impl HwDeviceContext {
    pub fn new(device_type: &str) -> Result<Self> {
        let type_cstr = CString::new(device_type)
            .with_context(|| format!("Invalid device type name: {}", device_type))?;

        let hw_type = unsafe { ffmpeg::ffi::av_hwdevice_find_type_by_name(type_cstr.as_ptr()) };

        if hw_type == ffmpeg::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_NONE {
            bail!(
                "Hardware device type '{}' not supported by FFmpeg",
                device_type
            );
        }

        let mut device_ctx: *mut ffmpeg::ffi::AVBufferRef = ptr::null_mut();
        let ret = unsafe {
            ffmpeg::ffi::av_hwdevice_ctx_create(
                &mut device_ctx,
                hw_type,
                ptr::null(),
                ptr::null_mut(),
                0,
            )
        };

        if ret < 0 {
            bail!(
                "Failed to create hardware device context for '{}' (error {})",
                device_type,
                ret
            );
        }

        Ok(Self {
            ctx: device_ctx,
            device_type: hw_type,
        })
    }

    pub fn device_type(&self) -> ffmpeg::ffi::AVHWDeviceType {
        self.device_type
    }

    pub fn as_ptr(&self) -> *mut ffmpeg::ffi::AVBufferRef {
        self.ctx
    }
}

impl Drop for HwDeviceContext {
    fn drop(&mut self) {
        if !self.ctx.is_null() {
            unsafe {
                ffmpeg::ffi::av_buffer_unref(&mut self.ctx);
            }
        }
    }
}

pub fn find_hw_format_for_codec(
    codec: &ffmpeg::decoder::Video,
    device_type: ffmpeg::ffi::AVHWDeviceType,
) -> Option<ffmpeg::ffi::AVPixelFormat> {
    unsafe {
        let codec_ptr = (*codec.as_ptr()).codec;
        for i in 0.. {
            let config = ffmpeg::ffi::avcodec_get_hw_config(codec_ptr, i);
            if config.is_null() {
                break;
            }
            let methods: i32 = (*config).methods;
            let config_device_type = (*config).device_type;
            let pix_fmt = (*config).pix_fmt;
            let hw_device_flag: i32 = ffmpeg::ffi::AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX as i32;

            if (methods & hw_device_flag) != 0 && config_device_type == device_type {
                return Some(pix_fmt);
            }
        }
    }
    None
}

pub fn try_create_hw_context(
    codec: &ffmpeg::decoder::Video,
) -> Option<(HwDeviceContext, ffmpeg::ffi::AVPixelFormat)> {
    for device_type in &["d3d11va", "dxva2"] {
        if let Ok(hw_ctx) = HwDeviceContext::new(device_type) {
            if let Some(hw_pix_fmt) = find_hw_format_for_codec(codec, hw_ctx.device_type()) {
                tracing::info!(
                    "Using hardware decode: {} (pixel format {:?})",
                    device_type,
                    hw_pix_fmt
                );
                return Some((hw_ctx, hw_pix_fmt));
            }
        }
    }
    tracing::info!("No hardware decode available, using software decode");
    None
}

pub unsafe fn transfer_frame_to_sw(
    hw_frame: &ffmpeg::util::frame::video::Video,
    sw_frame: &mut ffmpeg::util::frame::video::Video,
) -> Result<()> {
    let ret = ffmpeg::ffi::av_hwframe_transfer_data(sw_frame.as_mut_ptr(), hw_frame.as_ptr(), 0);
    if ret < 0 {
        bail!("Failed to transfer frame from GPU to CPU (error {})", ret);
    }
    Ok(())
}
