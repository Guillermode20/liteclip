use std::ffi::CString;

use anyhow::{Context, Result};
use ffmpeg::format::Pixel;
use ffmpeg_next as ffmpeg;
use tracing::{info, warn};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11DeviceContext4,
    ID3D11Resource, ID3D11Texture2D, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows_core::Interface;

use crate::config::{QualityPreset, RateControl};

use super::context::{AvD3d11vaDeviceContext, D3d11HardwareContext};
use super::FfmpegEncoder;

impl FfmpegEncoder {
    pub(super) fn apply_amf_options(&self, options: &mut ffmpeg::Dictionary<'_>, bitrate: usize) {
        let bitrate_bps = bitrate.to_string();
        let peak_bitrate_bps = self.peak_bitrate_bps().to_string();
        let (
            preanalysis,
            vbaq,
            rc_lookahead,
            me_half_pel,
            me_quarter_pel,
            high_motion_quality_boost,
        ) = match self.config.quality_preset {
            QualityPreset::Performance => ("0", "0", "0", "1", "0", "0"),
            QualityPreset::Balanced => ("0", "1", "0", "1", "1", "0"),
            QualityPreset::Quality => ("0", "1", "0", "1", "1", "1"),
        };

        options.set("usage", "lowlatency");
        options.set("quality", self.amf_quality());
        options.set("rc", self.amf_rc_mode());
        options.set("aud", "1");
        options.set("bf", "0");
        options.set("header_insertion_mode", "idr");
        options.set("gops_per_idr", "1");
        options.set("pa_adaptive_mini_gop", "0");
        options.set("preanalysis", preanalysis);
        options.set("vbaq", vbaq);
        options.set("rc_lookahead", rc_lookahead);
        options.set("max_qp_delta", "4");
        options.set("filler_data", "0");
        options.set("me_half_pel", me_half_pel);
        options.set("me_quarter_pel", me_quarter_pel);
        options.set(
            "high_motion_quality_boost_enable",
            high_motion_quality_boost,
        );
        options.set("min_qp_i", "16");
        options.set("max_qp_i", "48");
        options.set("min_qp_p", "18");
        options.set("max_qp_p", "48");
        options.set("profile_tier", "high");
        options.set("b", &bitrate_bps);
        options.set("max_bitrate", &peak_bitrate_bps);
        options.set("maxrate", &peak_bitrate_bps);
        options.set("bufsize", &bitrate_bps);

        if matches!(self.config.rate_control, RateControl::Cbr) {
            options.set("minrate", &bitrate_bps);
        }

        if matches!(self.config.rate_control, RateControl::Cq) {
            options.set("qvbr_quality_level", &self.cq_value().to_string());
        }
    }

    pub(super) fn create_d3d11_hardware_context(
        &self,
        gpu_frame: &crate::capture::D3d11Frame,
        width: u32,
        height: u32,
    ) -> Result<D3d11HardwareContext> {
        unsafe {
            let dxgi_device: IDXGIDevice = gpu_frame
                .device
                .cast()
                .context("Failed to get IDXGIDevice from capture D3D11 device")?;
            let adapter = dxgi_device
                .GetAdapter()
                .context("Failed to get DXGI adapter from capture device")?;
            let adapter_typed: windows::Win32::Graphics::Dxgi::IDXGIAdapter =
                adapter.cast().context("Failed to cast DXGI adapter")?;

            let feature_levels = [
                windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_1,
                windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0,
            ];
            let mut encoder_device_opt: Option<ID3D11Device> = None;
            let mut encoder_context_opt: Option<ID3D11DeviceContext> = None;
            D3D11CreateDevice(
                Some(&adapter_typed),
                windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN,
                windows::Win32::Foundation::HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&feature_levels),
                D3D11_SDK_VERSION,
                Some(&mut encoder_device_opt),
                None,
                Some(&mut encoder_context_opt),
            )
            .ok()
            .context("Failed to create encoder D3D11 device")?;
            let encoder_device = encoder_device_opt.context("Encoder D3D11 device is null")?;
            let encoder_context = encoder_context_opt.context("Encoder D3D11 context is null")?;

            info!("Encoder using separate D3D11 device (isolated from capture thread)");

            let device_type_name = CString::new("d3d11va").expect("static string");
            let device_type = ffmpeg::ffi::av_hwdevice_find_type_by_name(device_type_name.as_ptr());
            if device_type == ffmpeg::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_NONE {
                anyhow::bail!("FFmpeg D3D11VA hardware device type is unavailable");
            }

            let device_ctx_ref = ffmpeg::ffi::av_hwdevice_ctx_alloc(device_type);
            if device_ctx_ref.is_null() {
                anyhow::bail!("Failed to allocate FFmpeg D3D11 device context");
            }

            let hw_device_ctx = (*device_ctx_ref).data as *mut ffmpeg::ffi::AVHWDeviceContext;
            let d3d11_device_ctx = (*hw_device_ctx).hwctx as *mut AvD3d11vaDeviceContext;

            let ffmpeg_device = encoder_device.clone();
            let ffmpeg_context = encoder_context.clone();
            (*d3d11_device_ctx).device = ffmpeg_device.as_raw() as *mut _;
            (*d3d11_device_ctx).device_context = ffmpeg_context.as_raw() as *mut _;

            let init_result = ffmpeg::ffi::av_hwdevice_ctx_init(device_ctx_ref);
            if init_result < 0 {
                let mut device_ctx_ref = device_ctx_ref;
                ffmpeg::ffi::av_buffer_unref(&mut device_ctx_ref);
                anyhow::bail!(
                    "Failed to initialize FFmpeg D3D11 device context: {}",
                    init_result
                );
            }

            let pool_sizes: &[i32] = &[0, 4, 2];
            let sw_format = self.hardware_frame_sw_format();
            let mut frames_ctx_ref_result = Err(anyhow::anyhow!("no pool sizes tried"));
            for &pool_size in pool_sizes {
                match Self::create_hw_frames_ctx_with_pool_size(
                    device_ctx_ref,
                    width,
                    height,
                    sw_format,
                    pool_size,
                ) {
                    Ok(frames_ctx_ref) => {
                        if pool_size == 0 {
                            info!("Initialized FFmpeg D3D11 frame context with dynamic pool");
                        } else {
                            info!(
                                "Initialized FFmpeg D3D11 frame pool with {} surfaces",
                                pool_size
                            );
                        }
                        frames_ctx_ref_result = Ok(frames_ctx_ref);
                        break;
                    }
                    Err(error) => {
                        if pool_size == 0 {
                            frames_ctx_ref_result = Err(error);
                        } else {
                            warn!(
                                "Failed to initialize {}-surface FFmpeg D3D11 frame pool, trying smaller: {}",
                                pool_size, error
                            );
                        }
                    }
                }
            }
            let frames_ctx_ref = match frames_ctx_ref_result {
                Ok(frames_ctx_ref) => frames_ctx_ref,
                Err(e) => {
                    let mut device_ctx_ref = device_ctx_ref;
                    ffmpeg::ffi::av_buffer_unref(&mut device_ctx_ref);
                    return Err(e);
                }
            };

            let reusable_hw_frame = ffmpeg::ffi::av_frame_alloc();
            if reusable_hw_frame.is_null() {
                let mut frames_ctx_ref = frames_ctx_ref;
                let mut device_ctx_ref = device_ctx_ref;
                ffmpeg::ffi::av_buffer_unref(&mut frames_ctx_ref);
                ffmpeg::ffi::av_buffer_unref(&mut device_ctx_ref);
                anyhow::bail!("Failed to allocate reusable FFmpeg hardware frame");
            }

            Ok(D3d11HardwareContext {
                device_ctx_ref,
                frames_ctx_ref,
                copy_context: encoder_context,
                reusable_hw_frame,
                encoder_device: Some(encoder_device),
                encoder_fence: None,
                cached_shared_textures: Vec::with_capacity(4),
                is_shared_device: false,
            })
        }
    }

    pub(super) fn init_hardware_encoder(
        &mut self,
        gpu_frame: &crate::capture::D3d11Frame,
        width: u32,
        height: u32,
    ) -> Result<()> {
        let codec_name = self.config.ffmpeg_codec_name();
        let codec = ffmpeg::encoder::find_by_name(codec_name)
            .context(format!("Failed to find AMF encoder: {}", codec_name))?;

        let (out_w, out_h) = if self.config.use_native_resolution {
            (width, height)
        } else {
            self.config.resolution
        };

        // Reuse capture device (Optimization 4)
        let hw_context = self.create_d3d11_hardware_context_from_device(&gpu_frame.device, out_w, out_h)
            .context("Failed to create hardware context from shared device")?;

        let mut encoder = ffmpeg::codec::context::Context::new_with_codec(codec)
            .encoder()
            .video()
            .context("Failed to create encoder context")?;

        encoder.set_width(out_w);
        encoder.set_height(out_h);
        encoder.set_format(Pixel::D3D11);
        encoder.set_time_base((1, self.config.framerate as i32));
        encoder.set_frame_rate(Some((self.config.framerate as i32, 1)));
        Self::apply_bt709_encoder_metadata(&mut encoder);

        let bitrate = self.bitrate_bps();
        encoder.set_bit_rate(bitrate);
        encoder.set_max_bit_rate(self.peak_bitrate_bps());
        encoder.set_gop(self.config.keyframe_interval_frames());

        unsafe {
            (*encoder.as_mut_ptr()).hw_device_ctx =
                ffmpeg::ffi::av_buffer_ref(hw_context.device_ctx_ref);
            (*encoder.as_mut_ptr()).hw_frames_ctx =
                ffmpeg::ffi::av_buffer_ref(hw_context.frames_ctx_ref);
        }

        let mut options = ffmpeg::Dictionary::new();
        options.set("bf", "0");
        self.apply_codec_specific_options(&mut options, bitrate)?;

        let encoder = encoder
            .open_with(options)
            .context("Failed to open AMF encoder")?;

        self.encoder = Some(encoder);
        self.hw_context = Some(hw_context);
        self.scaler = None;
        self.src_frame = None;
        self.dst_frame = None;
        self.last_input_res = (width, height);
        self.pending_packet_timestamps.clear();

        info!(
            "AMF hardware encoder initialized (shared device): {} ({}x{})",
            codec_name, out_w, out_h
        );

        Ok(())
    }

    pub(super) fn encode_gpu_frame(
        &mut self,
        _frame: &crate::capture::CapturedFrame,
        gpu_frame: &crate::capture::D3d11Frame,
        pts: i64,
        gop: i64,
    ) -> Result<()> {
        let Some(ref mut encoder) = self.encoder else {
            return Ok(());
        };
        let Some(ref mut hw_context) = self.hw_context else {
            return Ok(());
        };

        unsafe {
            let hw_frame = hw_context.reusable_hw_frame;
            ffmpeg::ffi::av_frame_unref(hw_frame);

            let get_buffer_res = ffmpeg::ffi::av_hwframe_get_buffer(hw_context.frames_ctx_ref, hw_frame, 0);
            if get_buffer_res < 0 {
                anyhow::bail!("Failed to get hardware frame buffer: {}", get_buffer_res);
            }

            // For shared device, try to use the source texture directly
            (*hw_frame).data[0] = gpu_frame.texture.as_raw() as *mut _;
            (*hw_frame).data[1] = std::ptr::null_mut();
            (*hw_frame).data[2] = std::ptr::null_mut();
            (*hw_frame).data[3] = std::ptr::null_mut();

            if !hw_context.is_shared_device {
                // Cross-device logic (OpenSharedResource + Fence wait)
                let texture_ptr = (*hw_frame).data[0] as *mut ID3D11Texture2D;
                let dst_texture = &*texture_ptr;
                let dest_subresource = (*hw_frame).data[1] as usize as u32;
                
                let shared_texture = if let Some(found) = hw_context
                    .cached_shared_textures
                    .iter()
                    .find(|(h, _)| *h == gpu_frame.shared_handle)
                    .map(|(_, t)| t)
                {
                    found.clone()
                } else {
                    let mut opened_opt: Option<ID3D11Texture2D> = None;
                    if let Some(ref encoder_device) = hw_context.encoder_device {
                        encoder_device
                            .OpenSharedResource(gpu_frame.shared_handle, &mut opened_opt)
                            .context("Failed to open shared texture on encoder device")?;
                    } else {
                        anyhow::bail!("No encoder device available for cross-device texture sharing");
                    }
                    let opened = opened_opt.context("OpenSharedResource returned null")?;
                    hw_context
                        .cached_shared_textures
                        .push((gpu_frame.shared_handle, opened.clone()));
                    opened
                };

                if let (Some(ref fence), Some(_handle)) =
                    (&hw_context.encoder_fence, gpu_frame.fence_shared_handle)
                {
                    let ctx4: windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext4 = hw_context
                        .copy_context
                        .cast()
                        .context("Failed to get ID3D11DeviceContext4 for encoder wait")?;
                    ctx4.Wait(fence, gpu_frame.fence_value)
                        .context("Failed to wait on shared fence in encoder")?;
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

            (*hw_frame).pts = pts;
            if gop > 0 && self.frame_count % gop == 0 {
                (*hw_frame).pict_type = ffmpeg::picture::Type::I.into();
                (*hw_frame).key_frame = 1;
            } else {
                (*hw_frame).pict_type = ffmpeg::picture::Type::None.into();
                (*hw_frame).key_frame = 0;
            }
            Self::apply_bt709_raw_frame_metadata(hw_frame);

            let send_result = ffmpeg::ffi::avcodec_send_frame(encoder.as_mut_ptr(), hw_frame);
            if send_result < 0 {
                anyhow::bail!("Failed to send D3D11 frame to encoder: {}", send_result);
            }
        }

        Ok(())
    }
}
