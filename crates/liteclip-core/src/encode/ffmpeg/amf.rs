//! AMD AMF hardware encoding. Primary reference implementation for the D3D11 GPU frame path alongside
//! [`super::nvenc`](crate::encode::ffmpeg::nvenc) and [`super::qsv`](crate::encode::ffmpeg::qsv).
//! **Contributor checklist:** See the module-level docs on [`crate::encode::ffmpeg`].

use ffmpeg::format::Pixel;
use ffmpeg_next as ffmpeg;
use tracing::info;

use crate::config::{QualityPreset, RateControl};
use crate::encode::{EncodeError, EncodeResult};

use super::FfmpegEncoder;

/// Helper to write integer to stack buffer without heap allocation.
#[inline]
fn write_int_to_buffer<'a>(mut val: usize, buf: &'a mut [u8; 16]) -> &'a str {
    if val == 0 {
        buf[0] = b'0';
        return unsafe { std::str::from_utf8_unchecked(&buf[..1]) };
    }
    
    let mut pos = 15;
    while val > 0 {
        buf[pos] = b'0' + (val % 10) as u8;
        val /= 10;
        pos -= 1;
    }
    
    unsafe { std::str::from_utf8_unchecked(&buf[pos + 1..]) }
}

impl FfmpegEncoder {
    pub(super) fn apply_amf_options(&self, options: &mut ffmpeg::Dictionary<'_>, bitrate: usize) {
        let mut bitrate_str = [0u8; 16];
        let bitrate_bps = write_int_to_buffer(bitrate, &mut bitrate_str);
        
        let mut peak_str = [0u8; 16];
        let peak_bitrate_bps = write_int_to_buffer(self.peak_bitrate_bps(), &mut peak_str);
        
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
        options.set("b", bitrate_bps);
        options.set("max_bitrate", peak_bitrate_bps);
        options.set("maxrate", peak_bitrate_bps);
        options.set("bufsize", bitrate_bps);

        if matches!(self.config.rate_control, RateControl::Cbr) {
            options.set("minrate", bitrate_bps);
        }

        if matches!(self.config.rate_control, RateControl::Cq) {
            let mut cq_str = [0u8; 16];
            let cq_val = write_int_to_buffer(self.cq_value() as usize, &mut cq_str);
            options.set("qvbr_quality_level", cq_val);
        }
    }

    pub(super) fn init_amf_hardware_encoder(
        &mut self,
        gpu_frame: &crate::media::D3d11Frame,
        width: u32,
        height: u32,
    ) -> EncodeResult<()> {
        let codec_name = self.config.ffmpeg_codec_name();
        let codec = ffmpeg::encoder::find_by_name(codec_name).ok_or_else(|| {
            EncodeError::msg(format!("Failed to find AMF encoder: {}", codec_name))
        })?;

        let (out_w, out_h) = if self.config.use_native_resolution {
            (width, height)
        } else {
            self.config.resolution
        };

        // Reuse capture device (Optimization 4)
        let hw_context =
            self.create_d3d11_hardware_context_from_device(&gpu_frame.device, out_w, out_h)?;

        let mut encoder = ffmpeg::codec::context::Context::new_with_codec(codec)
            .encoder()
            .video()
            .map_err(EncodeError::ffmpeg)?;

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

        let encoder = encoder.open_with(options)?;

        self.encoder = Some(encoder);
        self.hw_context = Some(hw_context);
        self.scaler = None;
        self.src_frame = None;
        self.dst_frame = None;
        self.last_input_res = (width, height);
        self.pending_packet_timestamps.clear();
        self.clear_gpu_duplicate_state();

        info!(
            "AMF hardware encoder initialized (shared device): {} ({}x{})",
            codec_name, out_w, out_h
        );

        Ok(())
    }
}

impl FfmpegEncoder {
    pub(super) fn encode_amf_gpu_frame(
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
            let hw_frame = hw_context.reusable_hw_frame;
            Self::prepare_hw_frame(hw_context, hw_frame, gpu_frame)?;

            (*hw_frame).pts = pts;
            if gop > 0 && self.encoder_frame_count % gop == 0 {
                (*hw_frame).pict_type = ffmpeg::picture::Type::I.into();
                (*hw_frame).key_frame = 1;
            } else {
                (*hw_frame).pict_type = ffmpeg::picture::Type::None.into();
                (*hw_frame).key_frame = 0;
            }
            Self::apply_bt709_raw_frame_metadata(hw_frame);

            let send_result = ffmpeg::ffi::avcodec_send_frame(encoder.as_mut_ptr(), hw_frame);
            if send_result < 0 {
                return Err(EncodeError::msg(format!(
                    "Failed to send D3D11 frame to encoder: {}",
                    send_result
                )));
            }
        }

        Ok(())
    }
}
