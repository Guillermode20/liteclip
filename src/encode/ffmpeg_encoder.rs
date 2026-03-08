use super::{EncodedPacket, Encoder, EncoderConfig, StreamType};
use crate::capture::GpuTextureFormat;
use crate::config::{EncoderType, QualityPreset, RateControl};
use anyhow::{Context, Result};
use crossbeam::channel::{bounded, Receiver, Sender};
use ffmpeg::format::Pixel;
use ffmpeg_next as ffmpeg;
use std::collections::VecDeque;
use std::ffi::{c_void, CString};
use tracing::{info, warn};
use windows::Win32::Graphics::Direct3D11::{ID3D11DeviceContext, ID3D11Resource, ID3D11Texture2D};
use windows_core::Interface;

#[repr(C)]
struct AvD3d11vaDeviceContext {
    device: *mut c_void,
    device_context: *mut c_void,
    video_device: *mut c_void,
    video_context: *mut c_void,
    lock: Option<unsafe extern "C" fn(*mut c_void)>,
    unlock: Option<unsafe extern "C" fn(*mut c_void)>,
    lock_ctx: *mut c_void,
}

struct D3d11HardwareContext {
    device_ctx_ref: *mut ffmpeg::ffi::AVBufferRef,
    frames_ctx_ref: *mut ffmpeg::ffi::AVBufferRef,
    copy_context: ID3D11DeviceContext,
    reusable_hw_frame: *mut ffmpeg::ffi::AVFrame,
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

pub struct FfmpegEncoder {
    config: EncoderConfig,
    encoder: Option<ffmpeg::encoder::Video>,
    packet_tx: Sender<EncodedPacket>,
    packet_rx: Receiver<EncodedPacket>,
    frame_count: i64,
    running: bool,
    scaler: Option<ffmpeg::software::scaling::Context>,
    src_frame: Option<ffmpeg::util::frame::video::Video>,
    dst_frame: Option<ffmpeg::util::frame::video::Video>,
    hw_context: Option<D3d11HardwareContext>,
    last_input_res: (u32, u32),
    pending_packet_timestamps: VecDeque<i64>,
}

unsafe impl Send for FfmpegEncoder {}

impl FfmpegEncoder {
    pub fn new(config: &EncoderConfig) -> Result<Self> {
        let (tx, rx) = bounded(128);
        Ok(Self {
            config: config.clone(),
            encoder: None,
            packet_tx: tx,
            packet_rx: rx,
            frame_count: 0,
            running: false,
            scaler: None,
            src_frame: None,
            dst_frame: None,
            hw_context: None,
            last_input_res: (0, 0),
            pending_packet_timestamps: VecDeque::with_capacity(256),
        })
    }

    /// Determine the output pixel format based on encoder type.
    /// NVENC supports BGRA directly, avoiding CPU scaling.
    /// AMF/QSV prefer NV12 for hardware efficiency.
    /// Auto falls back to NV12 as a safe default.
    fn encoder_pixel_format(&self) -> Pixel {
        match self.config.encoder_type {
            EncoderType::Nvenc => Pixel::BGRA, // NVENC accepts BGRA directly - no scaling needed
            EncoderType::Amf | EncoderType::Qsv | EncoderType::Auto => Pixel::NV12, // NV12 is native for hardware encoders
        }
    }

    fn supports_gpu_frames(&self) -> bool {
        self.config.supports_gpu_frame_transport()
    }

    fn bitrate_bps(&self) -> usize {
        (self.config.bitrate_mbps.max(1) * 1_000_000) as usize
    }

    fn peak_bitrate_bps(&self) -> usize {
        match self.config.rate_control {
            RateControl::Cbr => self.bitrate_bps(),
            RateControl::Vbr | RateControl::Cq => self.bitrate_bps().saturating_mul(2),
        }
    }

    fn cq_value(&self) -> u8 {
        self.config
            .quality_value
            .unwrap_or(match self.config.quality_preset {
                QualityPreset::Performance => 28,
                QualityPreset::Balanced => 23,
                QualityPreset::Quality => 19,
            })
    }

    fn nvenc_preset(&self) -> &'static str {
        match self.config.quality_preset {
            QualityPreset::Performance => "p3",
            QualityPreset::Balanced => "p5",
            QualityPreset::Quality => "p7",
        }
    }

    fn nvenc_tune(&self) -> &'static str {
        match self.config.quality_preset {
            QualityPreset::Performance => "ull",
            QualityPreset::Balanced => "ll",
            QualityPreset::Quality => "hq",
        }
    }

    fn qsv_preset(&self) -> &'static str {
        match self.config.quality_preset {
            QualityPreset::Performance => "veryfast",
            QualityPreset::Balanced => "faster",
            QualityPreset::Quality => "medium",
        }
    }

    fn amf_quality(&self) -> &'static str {
        match self.config.quality_preset {
            QualityPreset::Performance => "speed",
            QualityPreset::Balanced => "balanced",
            QualityPreset::Quality => "quality",
        }
    }

    fn amf_rc_mode(&self) -> &'static str {
        match self.config.rate_control {
            RateControl::Cbr => "cbr",
            RateControl::Vbr | RateControl::Cq => "vbr_latency",
        }
    }

    fn next_encoder_pts(&self) -> i64 {
        self.frame_count
    }

    fn dequeue_packet_timestamp(&mut self, fallback: i64) -> i64 {
        self.pending_packet_timestamps
            .pop_front()
            .unwrap_or(fallback)
    }

    fn convert_length_prefixed_to_annex_b(data: &[u8]) -> Option<Vec<u8>> {
        if data.len() < 4 {
            return None;
        }

        if data.starts_with(&[0x00, 0x00, 0x00, 0x01]) || data.starts_with(&[0x00, 0x00, 0x01]) {
            return None;
        }

        let mut cursor = 0usize;
        let mut converted = Vec::with_capacity(data.len() + 16);

        while cursor + 4 <= data.len() {
            let nal_len = u32::from_be_bytes([
                data[cursor],
                data[cursor + 1],
                data[cursor + 2],
                data[cursor + 3],
            ]) as usize;
            cursor += 4;

            if nal_len == 0 || cursor + nal_len > data.len() {
                return None;
            }

            converted.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
            converted.extend_from_slice(&data[cursor..cursor + nal_len]);
            cursor += nal_len;
        }

        if cursor == data.len() && !converted.is_empty() {
            Some(converted)
        } else {
            None
        }
    }

    fn detect_keyframe(data: &[u8], packet_is_key: bool) -> bool {
        if data.is_empty() {
            return packet_is_key;
        }

        let mut i = 0usize;
        while i + 4 < data.len() && i < 100 {
            if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 0 && data[i + 3] == 1 {
                let nal_byte = data[i + 4];
                let hevc_type = (nal_byte >> 1) & 0x3f;
                // HEVC NAL types 19, 20 = IDR slice (keyframe)
                if hevc_type == 19 || hevc_type == 20 {
                    return true;
                }
                i += 4;
            } else if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
                let nal_byte = data[i + 3];
                let hevc_type = (nal_byte >> 1) & 0x3f;
                if hevc_type == 19 || hevc_type == 20 {
                    return true;
                }
                i += 3;
            } else {
                i += 1;
            }
        }

        packet_is_key
    }

    fn init_encoder(&mut self, width: u32, height: u32) -> Result<()> {
        let codec_name = self.config.ffmpeg_codec_name();
        let codec = ffmpeg::encoder::find_by_name(codec_name)
            .context(format!("Failed to find encoder: {}", codec_name))?;

        let encoder_ctx = ffmpeg::codec::context::Context::new_with_codec(codec);
        let mut encoder = encoder_ctx
            .encoder()
            .video()
            .context("Failed to create encoder context")?;

        let (out_w, out_h) = if self.config.use_native_resolution {
            (width, height)
        } else {
            self.config.resolution
        };

        encoder.set_width(out_w);
        encoder.set_height(out_h);
        let encoder_pix_fmt = self.encoder_pixel_format();
        encoder.set_format(encoder_pix_fmt);
        encoder.set_frame_rate(Some((self.config.framerate as i32, 1)));
        encoder.set_time_base((1, self.config.framerate as i32));

        let bitrate = self.bitrate_bps();
        encoder.set_bit_rate(bitrate);
        encoder.set_max_bit_rate(self.peak_bitrate_bps());
        encoder.set_gop(self.config.keyframe_interval_frames());

        let mut options = ffmpeg::Dictionary::new();
        options.set("bf", "0");

        match self.config.encoder_type {
            EncoderType::Nvenc => {
                let bitrate_bps = bitrate.to_string();
                let peak_bitrate_bps = self.peak_bitrate_bps().to_string();

                options.set("preset", self.nvenc_preset());
                options.set("tune", self.nvenc_tune());
                options.set("delay", "0");
                options.set("zerolatency", "1");
                options.set("strict_gop", "1");
                options.set("b_ref_mode", "disabled");
                options.set(
                    "rc",
                    match self.config.rate_control {
                        RateControl::Cbr => "cbr",
                        RateControl::Vbr | RateControl::Cq => "vbr",
                    },
                );
                options.set("b", &bitrate_bps);
                options.set("maxrate", &peak_bitrate_bps);
                options.set("bufsize", &bitrate_bps);

                if matches!(self.config.rate_control, RateControl::Cbr) {
                    options.set("minrate", &bitrate_bps);
                }
                if matches!(self.config.rate_control, RateControl::Cq) {
                    options.set("cq", &self.cq_value().to_string());
                }

                options.set("forced-idr", "1");
            }
            EncoderType::Amf => {
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
                    QualityPreset::Performance => ("0", "0", "0", "0", "0", "0"),
                    QualityPreset::Balanced => ("0", "0", "0", "1", "0", "0"),
                    QualityPreset::Quality => ("0", "1", "0", "1", "1", "0"),
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
                options.set("min_qp_i", "18");
                options.set("max_qp_i", "51");
                options.set("min_qp_p", "20");
                options.set("max_qp_p", "51");

                // HEVC-specific
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
            EncoderType::Qsv => {
                let bitrate_bps = bitrate.to_string();
                let peak_bitrate_bps = self.peak_bitrate_bps().to_string();

                options.set("preset", self.qsv_preset());
                options.set("look_ahead", "0");
                options.set(
                    "rc",
                    match self.config.rate_control {
                        RateControl::Cbr => "cbr",
                        RateControl::Vbr | RateControl::Cq => "vbr",
                    },
                );
                options.set("b", &bitrate_bps);
                options.set("maxrate", &peak_bitrate_bps);
                options.set("bufsize", &bitrate_bps);
            }
            EncoderType::Auto => {
                // Should not reach here - Auto is resolved before init
                anyhow::bail!("Auto encoder type should be resolved before init");
            }
        }

        let encoder = encoder
            .open_with(options)
            .context("Failed to open encoder")?;

        self.encoder = Some(encoder);

        // Determine if we need scaling:
        // 1. For non-BGRA formats (AMF/QSV need NV12), always scale
        // 2. For BGRA (NVENC), scale if input dimensions differ from output dimensions
        //    (happens when GPU scaling failed and we're doing CPU-side fallback)
        let needs_scaling = encoder_pix_fmt != Pixel::BGRA
            || (!self.config.use_native_resolution && (out_w != width || out_h != height));

        self.src_frame = Some(ffmpeg::util::frame::video::Video::new(
            Pixel::BGRA,
            width,
            height,
        ));

        if needs_scaling {
            self.dst_frame = Some(ffmpeg::util::frame::video::Video::new(
                encoder_pix_fmt,
                out_w,
                out_h,
            ));
            self.scaler = Some(
                ffmpeg::software::scaling::Context::get(
                    Pixel::BGRA,
                    width,
                    height,
                    encoder_pix_fmt,
                    out_w,
                    out_h,
                    // NEAREST_NEIGHBOR is much faster than FAST_BILINEAR for realtime capture
                    // Quality difference is negligible for screen content
                    ffmpeg::software::scaling::flag::Flags::POINT,
                )
                .context("Failed to create scaler context")?,
            );
            info!(
                "Native FFmpeg encoder initialized: {} ({}x{}) with NV12 scaling (fast)",
                codec_name, out_w, out_h
            );
        } else {
            // NVENC with BGRA - no scaling needed, use src_frame as dst_frame
            self.dst_frame = Some(ffmpeg::util::frame::video::Video::new(
                Pixel::BGRA,
                out_w,
                out_h,
            ));
            self.scaler = None; // No scaling needed
            info!(
                "Native FFmpeg encoder initialized: {} ({}x{}) with BGRA (no scaling)",
                codec_name, out_w, out_h
            );
        }

        self.last_input_res = (width, height);
        self.pending_packet_timestamps.clear();
        Ok(())
    }

    fn create_d3d11_hardware_context(
        &self,
        gpu_frame: &crate::capture::D3d11Frame,
        width: u32,
        height: u32,
    ) -> Result<D3d11HardwareContext> {
        unsafe {
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

            let device = gpu_frame.device.clone();
            let immediate_context = gpu_frame
                .device
                .GetImmediateContext()
                .context("Failed to get D3D11 immediate context for encoder")?;
            let ffmpeg_device = device.clone();
            let ffmpeg_context = immediate_context.clone();
            (*d3d11_device_ctx).device = ffmpeg_device.as_raw() as *mut _;
            (*d3d11_device_ctx).device_context = ffmpeg_context.as_raw() as *mut _;
            std::mem::forget(ffmpeg_device);
            std::mem::forget(ffmpeg_context);

            let init_result = ffmpeg::ffi::av_hwdevice_ctx_init(device_ctx_ref);
            if init_result < 0 {
                let mut device_ctx_ref = device_ctx_ref;
                ffmpeg::ffi::av_buffer_unref(&mut device_ctx_ref);
                anyhow::bail!(
                    "Failed to initialize FFmpeg D3D11 device context: {}",
                    init_result
                );
            }

            let frames_ctx_ref = ffmpeg::ffi::av_hwframe_ctx_alloc(device_ctx_ref);
            if frames_ctx_ref.is_null() {
                let mut device_ctx_ref = device_ctx_ref;
                ffmpeg::ffi::av_buffer_unref(&mut device_ctx_ref);
                anyhow::bail!("Failed to allocate FFmpeg D3D11 frame context");
            }

            let frames_ctx = (*frames_ctx_ref).data as *mut ffmpeg::ffi::AVHWFramesContext;
            (*frames_ctx).format = Pixel::D3D11.into();
            (*frames_ctx).sw_format = Pixel::NV12.into();
            (*frames_ctx).width = width as i32;
            (*frames_ctx).height = height as i32;
            // Use 2 to avoid D3D11 array texture limits (ArraySize>2 fails with RENDER_TARGET on some drivers)
            (*frames_ctx).initial_pool_size = 2;

            let init_frames_result = ffmpeg::ffi::av_hwframe_ctx_init(frames_ctx_ref);
            if init_frames_result < 0 {
                let mut frames_ctx_ref = frames_ctx_ref;
                let mut device_ctx_ref = device_ctx_ref;
                ffmpeg::ffi::av_buffer_unref(&mut frames_ctx_ref);
                ffmpeg::ffi::av_buffer_unref(&mut device_ctx_ref);
                anyhow::bail!(
                    "Failed to initialize FFmpeg D3D11 frame context: {}",
                    init_frames_result
                );
            }

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
                copy_context: immediate_context,
                reusable_hw_frame,
            })
        }
    }

    fn init_hardware_encoder(
        &mut self,
        gpu_frame: &crate::capture::D3d11Frame,
        width: u32,
        height: u32,
    ) -> Result<()> {
        let codec_name = self.config.ffmpeg_codec_name();
        let codec = ffmpeg::encoder::find_by_name(codec_name)
            .context(format!("Failed to find encoder: {}", codec_name))?;

        let encoder_ctx = ffmpeg::codec::context::Context::new_with_codec(codec);
        let mut encoder = encoder_ctx
            .encoder()
            .video()
            .context("Failed to create encoder context")?;

        let (out_w, out_h) = if self.config.use_native_resolution {
            (width, height)
        } else {
            self.config.resolution
        };

        encoder.set_width(out_w);
        encoder.set_height(out_h);
        encoder.set_format(Pixel::D3D11);
        encoder.set_frame_rate(Some((self.config.framerate as i32, 1)));
        encoder.set_time_base((1, self.config.framerate as i32));

        let bitrate = self.bitrate_bps();
        encoder.set_bit_rate(bitrate);
        encoder.set_max_bit_rate(self.peak_bitrate_bps());
        encoder.set_gop(self.config.keyframe_interval_frames());

        let hw_context = self.create_d3d11_hardware_context(gpu_frame, out_w, out_h)?;
        unsafe {
            (*encoder.as_mut_ptr()).hw_frames_ctx =
                ffmpeg::ffi::av_buffer_ref(hw_context.frames_ctx_ref);
            if (*encoder.as_mut_ptr()).hw_frames_ctx.is_null() {
                anyhow::bail!("Failed to reference FFmpeg D3D11 frame context");
            }
        }

        let mut options = ffmpeg::Dictionary::new();
        options.set("bf", "0");

        let bitrate_bps = bitrate.to_string();
        let peak_bitrate_bps = self.peak_bitrate_bps().to_string();
        options.set("usage", "lowlatency");
        options.set("quality", self.amf_quality());
        options.set("rc", self.amf_rc_mode());
        options.set("aud", "1");
        options.set("header_insertion_mode", "idr");
        options.set("gops_per_idr", "1");
        options.set("pa_adaptive_mini_gop", "0");
        options.set("preanalysis", "0");
        options.set("vbaq", "0");
        options.set("rc_lookahead", "0");
        options.set("filler_data", "0");
        options.set("profile_tier", "high");
        options.set("b", &bitrate_bps);
        options.set("max_bitrate", &peak_bitrate_bps);
        options.set("maxrate", &peak_bitrate_bps);
        options.set("bufsize", &bitrate_bps);

        let encoder = encoder
            .open_with(options)
            .context("Failed to open hardware encoder")?;

        self.encoder = Some(encoder);
        self.hw_context = Some(hw_context);
        self.scaler = None;
        self.src_frame = None;
        self.dst_frame = None;
        self.last_input_res = (width, height);
        self.pending_packet_timestamps.clear();
        info!(
            "Native FFmpeg encoder initialized: {} ({}x{}) with D3D11 hardware frames",
            codec_name, out_w, out_h
        );
        Ok(())
    }

    fn encode_gpu_frame(
        &mut self,
        frame: &crate::capture::CapturedFrame,
        gpu_frame: &crate::capture::D3d11Frame,
        encoder_pts: i64,
        gop: i64,
    ) -> Result<()> {
        let Some(ref mut encoder) = self.encoder else {
            return Ok(());
        };
        let Some(ref hw_context) = self.hw_context else {
            anyhow::bail!("Hardware encoder context is not initialized");
        };

        unsafe {
            let hw_frame = hw_context.reusable_hw_frame;
            if hw_frame.is_null() {
                anyhow::bail!("Failed to allocate FFmpeg hardware frame");
            }
            ffmpeg::ffi::av_frame_unref(hw_frame);

            let get_buffer_result =
                ffmpeg::ffi::av_hwframe_get_buffer(hw_context.frames_ctx_ref, hw_frame, 0);
            if get_buffer_result < 0 {
                anyhow::bail!(
                    "Failed to allocate FFmpeg hardware frame buffer: {}",
                    get_buffer_result
                );
            }

            let source_resource: ID3D11Resource = gpu_frame
                .texture
                .cast()
                .context("Failed to cast source GPU frame texture to resource")?;

            let raw_texture = (*hw_frame).data[0] as *mut c_void;
            let dest_texture = ID3D11Texture2D::from_raw_borrowed(&raw_texture)
                .context("FFmpeg D3D11 hardware frame did not expose a destination texture")?;
            let dest_resource: ID3D11Resource = dest_texture
                .cast()
                .context("Failed to cast destination GPU frame texture to resource")?;
            let dest_array_slice = (*hw_frame).data[1] as usize as u32;
            let dest_subresource = dest_array_slice;

            // FFmpeg D3D11 hardware frames expose a texture plus an array-slice index in
            // `data[1]`. Copying the whole resource can leave the encoder reading an untouched
            // slice, which shows up as a solid green output even though encoding succeeds.
            hw_context.copy_context.CopySubresourceRegion(
                Some(&dest_resource),
                dest_subresource,
                0,
                0,
                0,
                Some(&source_resource),
                0,
                None,
            );
            hw_context.copy_context.Flush();

            (*hw_frame).pts = encoder_pts;
            (*hw_frame).width = frame.resolution.0 as i32;
            (*hw_frame).height = frame.resolution.1 as i32;
            (*hw_frame).pict_type = if gop > 0 && self.frame_count % gop == 0 {
                ffmpeg::picture::Type::I.into()
            } else {
                ffmpeg::picture::Type::None.into()
            };

            let send_result = ffmpeg::ffi::avcodec_send_frame(encoder.as_mut_ptr(), hw_frame);
            if send_result < 0 {
                anyhow::bail!("Failed to send D3D11 frame to encoder: {}", send_result);
            }
        }

        Ok(())
    }

    fn drain_encoder_packets(&mut self, fallback_timestamp: i64) -> Result<()> {
        let mut packets_data: Vec<(Vec<u8>, bool)> = Vec::with_capacity(8);

        if let Some(ref mut encoder) = self.encoder {
            let mut packet = ffmpeg::Packet::empty();
            while encoder.receive_packet(&mut packet).is_ok() {
                let data = packet.data().unwrap_or(&[]).to_vec();
                let packet_is_key = packet.is_key();
                packets_data.push((data, packet_is_key));
                packet = ffmpeg::Packet::empty();
            }
        }

        for (data, packet_is_key) in packets_data {
            let normalized_data = Self::convert_length_prefixed_to_annex_b(&data);
            let inspection_data = normalized_data.as_deref().unwrap_or(&data);
            let is_keyframe = Self::detect_keyframe(inspection_data, packet_is_key);
            let pts = self.dequeue_packet_timestamp(fallback_timestamp);

            if self.frame_count % 60 == 0 || is_keyframe {
                tracing::debug!("packet {}B keyframe={}", data.len(), is_keyframe);
            }

            let mut encoded_packet =
                EncodedPacket::new(data, pts, pts, is_keyframe, StreamType::Video);

            if !self.config.use_native_resolution {
                encoded_packet.resolution = Some(self.config.resolution);
            }

            if self.packet_tx.send(encoded_packet).is_err() {
                break;
            }
        }

        Ok(())
    }
}

impl Encoder for FfmpegEncoder {
    fn init(&mut self, _config: &EncoderConfig) -> Result<()> {
        self.running = true;
        Ok(())
    }

    fn encode_frame(&mut self, frame: &crate::capture::CapturedFrame) -> Result<()> {
        let gpu_frame = frame.d3d11.as_deref();

        // Check if we can use GPU frame transport
        // Only accept NV12 hardware frames - BGRA frames must use CPU path
        let can_use_gpu = gpu_frame.is_some()
            && self.supports_gpu_frames()
            && gpu_frame.unwrap().format == GpuTextureFormat::Nv12;
        let needs_transport_reinit = if can_use_gpu {
            self.hw_context.is_none()
        } else {
            self.hw_context.is_some()
        };

        if self.encoder.is_none()
            || self.last_input_res != (frame.resolution.0, frame.resolution.1)
            || needs_transport_reinit
        {
            if can_use_gpu {
                if let Some(gpu_frame) = gpu_frame {
                    if needs_transport_reinit && self.encoder.is_some() {
                        info!("GPU NV12 frames restored; reinitializing encoder for D3D11 transport");
                    } else {
                        info!(
                            "Initializing hardware encoder with D3D11 NV12 frames (GPU transport enabled)"
                        );
                    }
                    self.init_hardware_encoder(gpu_frame, frame.resolution.0, frame.resolution.1)?;
                }
            } else if gpu_frame.is_some() && self.supports_gpu_frames() {
                // GPU frame present but format is not NV12 - fall back to CPU path
                if let Some(gpu_frame) = gpu_frame {
                    warn!(
                        "GPU frame format is {:?}, expected NV12 for hardware encoder. Falling back to CPU path.",
                        gpu_frame.format
                    );
                }
                self.hw_context = None;
                self.init_encoder(frame.resolution.0, frame.resolution.1)?;
            } else {
                // No GPU frame or GPU transport not supported
                if needs_transport_reinit && self.encoder.is_some() && self.supports_gpu_frames() {
                    info!(
                        "GPU frame transport unavailable for current frame; reinitializing encoder for CPU input"
                    );
                }
                self.hw_context = None;
                self.init_encoder(frame.resolution.0, frame.resolution.1)?;
            }
        }

        let encoder_pts = self.next_encoder_pts();
        let gop = self.config.keyframe_interval_frames() as i64;
        self.pending_packet_timestamps.push_back(frame.timestamp);
        if self.pending_packet_timestamps.len() > 512 {
            self.pending_packet_timestamps.pop_front();
        }

        if can_use_gpu {
            if let Some(gpu_frame) = gpu_frame {
                self.encode_gpu_frame(frame, gpu_frame, encoder_pts, gop)?;
            }
        } else {
            let Some(ref mut encoder) = self.encoder else {
                return Ok(());
            };
            let Some(ref mut src_frame) = self.src_frame else {
                return Ok(());
            };
            let Some(ref mut dst_frame) = self.dst_frame else {
                return Ok(());
            };

            src_frame.data_mut(0).copy_from_slice(&frame.bgra);

            // For NVENC (scaler is None), use src_frame directly as dst_frame
            // For other encoders, run the software scaler
            if let Some(ref mut scaler) = self.scaler {
                scaler.run(src_frame, dst_frame)?;
            } else {
                // No scaling needed - copy src to dst directly
                dst_frame.data_mut(0).copy_from_slice(&frame.bgra);
            }

            dst_frame.set_pts(Some(encoder_pts));
            if gop > 0 && self.frame_count % gop == 0 {
                dst_frame.set_kind(ffmpeg::picture::Type::I);
            } else {
                dst_frame.set_kind(ffmpeg::picture::Type::None);
            }

            encoder
                .send_frame(dst_frame)
                .context("Failed to send frame to encoder")?;
        }

        self.drain_encoder_packets(frame.timestamp)?;
        self.frame_count += 1;
        Ok(())
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>> {
        if let Some(ref mut encoder) = self.encoder {
            encoder.send_eof().ok();
        }

        self.drain_encoder_packets(0)?;

        let mut packets = Vec::new();
        while let Ok(packet) = self.packet_rx.try_recv() {
            packets.push(packet);
        }

        self.running = false;
        Ok(packets)
    }

    fn packet_rx(&self) -> Receiver<EncodedPacket> {
        self.packet_rx.clone()
    }

    fn is_running(&self) -> bool {
        self.running
    }
}
