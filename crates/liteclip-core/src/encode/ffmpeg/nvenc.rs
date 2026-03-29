//! NVIDIA NVENC hardware encoding: D3D11-shared device, `Pixel::D3D11` transport, NV12 surfaces.
//!
//! Owns `FfmpegEncoder::{init_nvenc_hardware_encoder, encode_nvenc_gpu_frame}` and NVENC-specific
//! option keys in [`super::options`](crate::encode::ffmpeg::options).
//!
//! **Assumptions:** Capture supplies a D3D11 frame in the format expected for GPU transport (see
//! [`ResolvedEncoderConfig::gpu_texture_format`](crate::encode::encoder_mod::ResolvedEncoderConfig::gpu_texture_format)).
//!
//! **Verification:** NVIDIA GPU + driver with NVENC; FFmpeg with `hevc_nvenc` (or your codec) available.
//! Record with encoder set to NVENC and confirm no spurious CPU fallback in logs.
//!
//! **Contributor checklist:** See the module-level docs on [`crate::encode::ffmpeg`].

use ffmpeg_next as ffmpeg;
use tracing::info;

use crate::config::RateControl;
use crate::encode::{EncodeError, EncodeResult};

use super::FfmpegEncoder;

/// Helper to write integer to stack buffer without heap allocation.
/// Returns a &str slice of the written digits.
#[inline]
fn write_int_to_buffer(mut val: usize, buf: &mut [u8; 16]) -> &str {
    if val == 0 {
        buf[0] = b'0';
        // SAFETY: We just wrote a valid ASCII byte
        return unsafe { std::str::from_utf8_unchecked(&buf[..1]) };
    }

    let mut pos = 15;
    while val > 0 {
        buf[pos] = b'0' + (val % 10) as u8;
        val /= 10;
        pos -= 1;
    }

    // SAFETY: We only wrote ASCII digits
    unsafe { std::str::from_utf8_unchecked(&buf[pos + 1..]) }
}

impl FfmpegEncoder {
    pub(super) fn init_nvenc_hardware_encoder(
        &mut self,
        gpu_frame: &crate::media::D3d11Frame,
        width: u32,
        height: u32,
    ) -> EncodeResult<()> {
        let codec_name = self.config.ffmpeg_codec_name();
        let codec = ffmpeg::encoder::find_by_name(codec_name).ok_or_else(|| {
            EncodeError::msg(format!("Failed to find NVENC encoder: {}", codec_name))
        })?;

        let (out_w, out_h) = if self.config.use_native_resolution {
            (width, height)
        } else {
            self.config.resolution
        };

        // Reuse capture device (Direct transport)
        let hw_context =
            self.create_d3d11_hardware_context_from_device(&gpu_frame.device, out_w, out_h)?;

        let mut encoder = ffmpeg::codec::context::Context::new_with_codec(codec)
            .encoder()
            .video()
            .map_err(EncodeError::ffmpeg)?;

        encoder.set_width(out_w);
        encoder.set_height(out_h);
        encoder.set_format(ffmpeg::format::Pixel::D3D11);
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
            "NVENC hardware encoder initialized (shared device): {} ({}x{})",
            codec_name, out_w, out_h
        );

        Ok(())
    }

    pub(super) fn apply_nvenc_options(&self, options: &mut ffmpeg::Dictionary<'_>, bitrate: usize) {
        // Pre-allocate stack string for bitrate to avoid heap allocation
        // bitrate fits in i32 range, so max 11 digits
        let mut bitrate_str = [0u8; 16];
        let bitrate_bps = write_int_to_buffer(bitrate, &mut bitrate_str);

        let mut peak_str = [0u8; 16];
        let peak_bitrate_bps = write_int_to_buffer(self.peak_bitrate_bps(), &mut peak_str);

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
        options.set("b", bitrate_bps);
        options.set("maxrate", peak_bitrate_bps);
        options.set("bufsize", bitrate_bps);

        if matches!(self.config.rate_control, RateControl::Cbr) {
            options.set("minrate", bitrate_bps);
        }
        if matches!(self.config.rate_control, RateControl::Cq) {
            let mut cq_str = [0u8; 16];
            let cq_val = write_int_to_buffer(self.cq_value() as usize, &mut cq_str);
            options.set("cq", cq_val);
        }

        options.set("forced-idr", "1");
    }

    pub(super) fn encode_nvenc_gpu_frame(
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
                    "Failed to send D3D11 frame to NVENC: {}",
                    send_result
                )));
            }
        }

        Ok(())
    }
}
