//! Intel Quick Sync Video (QSV) hardware encoding: D3D11 device → derived QSV device/frames, QSV surfaces.
//!
//! Owns `FfmpegEncoder::{init_qsv_hardware_encoder, encode_qsv_gpu_frame}` and QSV-specific option keys
//! in [`super::options`](crate::encode::ffmpeg::options).
//!
//! **Assumptions:** Same D3D11/NV12 capture path as other hardware encoders; QSV is derived from the
//! shared D3D11 hardware context via FFmpeg’s `av_hwdevice_ctx_create_derived`.
//!
//! **Verification:** Intel iGPU/dGPU with working oneVPL/Media Stack; FFmpeg with QSV enabled. Record with
//! QSV selected and confirm mapping from D3D11 to QSV succeeds (watch for derive/map errors in logs).
//!
//! **Contributor checklist:** See the module-level docs on [`crate::encode::ffmpeg`].

use std::ffi::CString;

use ffmpeg_next as ffmpeg;
use tracing::info;

use crate::config::RateControl;
use crate::encode::{EncodeError, EncodeResult};

use super::FfmpegEncoder;

impl FfmpegEncoder {
    pub(super) fn init_qsv_hardware_encoder(
        &mut self,
        gpu_frame: &crate::media::D3d11Frame,
        width: u32,
        height: u32,
    ) -> EncodeResult<()> {
        let codec_name = self.config.ffmpeg_codec_name();
        let codec = ffmpeg::encoder::find_by_name(codec_name).ok_or_else(|| {
            EncodeError::msg(format!("Failed to find QSV encoder: {}", codec_name))
        })?;

        let (out_w, out_h) = if self.config.use_native_resolution {
            (width, height)
        } else {
            self.config.resolution
        };

        // Reuse capture device (Direct transport)
        let d3d11_hw_context =
            self.create_d3d11_hardware_context_from_device(&gpu_frame.device, out_w, out_h)?;

        unsafe {
            // Derive QSV device from D3D11 device
            let qsv_type_name = CString::new("qsv").expect("static string");
            let qsv_type = ffmpeg::ffi::av_hwdevice_find_type_by_name(qsv_type_name.as_ptr());
            if qsv_type == ffmpeg::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_NONE {
                return Err(EncodeError::msg(
                    "FFmpeg QSV hardware device type is unavailable",
                ));
            }

            let mut qsv_device_ctx_ref = std::ptr::null_mut();
            let derive_res = ffmpeg::ffi::av_hwdevice_ctx_create_derived(
                &mut qsv_device_ctx_ref,
                qsv_type,
                d3d11_hw_context.device_ctx_ref,
                0,
            );
            if derive_res < 0 {
                return Err(EncodeError::msg(format!(
                    "Failed to derive QSV device from D3D11: {}",
                    derive_res
                )));
            }

            // Derive QSV frames context from D3D11 frames context
            let mut qsv_frames_ctx_ref = ffmpeg::ffi::av_hwframe_ctx_alloc(qsv_device_ctx_ref);
            if qsv_frames_ctx_ref.is_null() {
                ffmpeg::ffi::av_buffer_unref(&mut qsv_device_ctx_ref);
                return Err(EncodeError::msg("Failed to allocate QSV frames context"));
            }

            let derive_frames_res = ffmpeg::ffi::av_hwframe_ctx_create_derived(
                &mut qsv_frames_ctx_ref,
                ffmpeg::ffi::AVPixelFormat::AV_PIX_FMT_QSV,
                qsv_device_ctx_ref,
                d3d11_hw_context.frames_ctx_ref,
                0,
            );
            if derive_frames_res < 0 {
                ffmpeg::ffi::av_buffer_unref(&mut qsv_frames_ctx_ref);
                ffmpeg::ffi::av_buffer_unref(&mut qsv_device_ctx_ref);
                return Err(EncodeError::msg(format!(
                    "Failed to derive QSV frames from D3D11: {}",
                    derive_frames_res
                )));
            }

            // Replace D3D11 references in context with QSV ones for the encoder
            // But keep D3D11 contexts for frame mapping/cleanup
            let mut encoder = ffmpeg::codec::context::Context::new_with_codec(codec)
                .encoder()
                .video()
                .map_err(|e| EncodeError::ffmpeg(e))?;

            encoder.set_width(out_w);
            encoder.set_height(out_h);
            encoder.set_format(ffmpeg::format::Pixel::QSV);
            encoder.set_time_base((1, self.config.framerate as i32));
            encoder.set_frame_rate(Some((self.config.framerate as i32, 1)));
            Self::apply_bt709_encoder_metadata(&mut encoder);

            let bitrate = self.bitrate_bps();
            encoder.set_bit_rate(bitrate);
            encoder.set_max_bit_rate(self.peak_bitrate_bps());
            encoder.set_gop(self.config.keyframe_interval_frames());

            (*encoder.as_mut_ptr()).hw_device_ctx = ffmpeg::ffi::av_buffer_ref(qsv_device_ctx_ref);
            (*encoder.as_mut_ptr()).hw_frames_ctx = ffmpeg::ffi::av_buffer_ref(qsv_frames_ctx_ref);

            // Update d3d11_hw_context to manage these QSV buffers too, or just unref them here if we don't need them beyond encoder life
            // Actually, we should store them so they stay alive as long as hw_context does.
            // But D3d11HardwareContext doesn't have slots for derived contexts.
            // Easiest: let encoder own them.
            ffmpeg::ffi::av_buffer_unref(&mut qsv_frames_ctx_ref);
            ffmpeg::ffi::av_buffer_unref(&mut qsv_device_ctx_ref);

            let mut options = ffmpeg::Dictionary::new();
            self.apply_codec_specific_options(&mut options, bitrate)?;

            let encoder = encoder.open_with(options)?;

            self.encoder = Some(encoder);
            self.hw_context = Some(d3d11_hw_context);
            self.scaler = None;
            self.src_frame = None;
            self.dst_frame = None;
            self.last_input_res = (width, height);
            self.pending_packet_timestamps.clear();

            info!(
                "QSV hardware encoder initialized (derived from D3D11): {} ({}x{})",
                codec_name, out_w, out_h
            );
        }

        Ok(())
    }

    pub(super) fn apply_qsv_options(&self, options: &mut ffmpeg::Dictionary<'_>, bitrate: usize) {
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

    pub(super) fn encode_qsv_gpu_frame(
        &mut self,
        _frame: &crate::media::CapturedFrame,
        gpu_frame: &crate::media::D3d11Frame,
        pts: i64,
        gop: i64,
    ) -> EncodeResult<()> {
        let Some(ref mut encoder) = self.encoder else {
            return Ok(());
        };
        let Some(ref mut hw_context) = self.hw_context else {
            return Ok(());
        };

        unsafe {
            let d3d11_frame = hw_context.reusable_hw_frame;
            Self::prepare_hw_frame(hw_context, d3d11_frame, gpu_frame)?;

            // Allocate a QSV surface
            let mut qsv_frame = ffmpeg::ffi::av_frame_alloc();
            if qsv_frame.is_null() {
                return Err(EncodeError::msg("Failed to allocate QSV frame"));
            }

            // Map D3D11 surface to QSV surface
            let map_res = ffmpeg::ffi::av_hwframe_map(
                qsv_frame,
                d3d11_frame,
                0, // AV_HWFRAME_MAP_DIRECT?
            );

            if map_res < 0 {
                ffmpeg::ffi::av_frame_free(&mut qsv_frame);
                return Err(EncodeError::msg(format!(
                    "Failed to map D3D11 surface to QSV: {}",
                    map_res
                )));
            }

            (*qsv_frame).pts = pts;
            if gop > 0 && self.frame_count % gop == 0 {
                (*qsv_frame).pict_type = ffmpeg::picture::Type::I.into();
                (*qsv_frame).key_frame = 1;
            } else {
                (*qsv_frame).pict_type = ffmpeg::picture::Type::None.into();
                (*qsv_frame).key_frame = 0;
            }
            Self::apply_bt709_raw_frame_metadata(qsv_frame);

            let send_result = ffmpeg::ffi::avcodec_send_frame(encoder.as_mut_ptr(), qsv_frame);
            ffmpeg::ffi::av_frame_free(&mut qsv_frame);

            if send_result < 0 {
                return Err(EncodeError::msg(format!(
                    "Failed to send mapped QSV frame to encoder: {}",
                    send_result
                )));
            }
        }

        Ok(())
    }
}
