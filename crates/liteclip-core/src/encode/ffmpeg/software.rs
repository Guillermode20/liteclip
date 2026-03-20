use bytes::Bytes;
use ffmpeg::format::Pixel;
use ffmpeg_next as ffmpeg;
use tracing::info;

use crate::encode::{EncodeError, EncodeResult};

use super::{EncodedPacket, FfmpegEncoder, StreamType};

impl FfmpegEncoder {
    pub(super) fn init_encoder(&mut self, width: u32, height: u32) -> EncodeResult<()> {
        let codec_name = self.config.ffmpeg_codec_name();
        let codec = ffmpeg::encoder::find_by_name(codec_name)
            .ok_or_else(|| EncodeError::msg(format!("Failed to find encoder: {}", codec_name)))?;

        let encoder_ctx = ffmpeg::codec::context::Context::new_with_codec(codec);
        let mut encoder = encoder_ctx
            .encoder()
            .video()
            .map_err(|e| EncodeError::ffmpeg(e))?;

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
        Self::apply_bt709_encoder_metadata(&mut encoder);

        let bitrate = self.bitrate_bps();
        encoder.set_bit_rate(bitrate);
        encoder.set_max_bit_rate(self.peak_bitrate_bps());
        encoder.set_gop(self.config.keyframe_interval_frames());

        let mut options = ffmpeg::Dictionary::new();
        options.set("bf", "0");
        self.apply_codec_specific_options(&mut options, bitrate)?;

        let encoder = encoder.open_with(options)?;

        self.encoder = Some(encoder);

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
                    ffmpeg::software::scaling::flag::Flags::POINT,
                )
                .map_err(|e| EncodeError::ffmpeg(e))?,
            );
            info!(
                "Native FFmpeg encoder initialized: {} ({}x{}) with NV12 scaling (fast)",
                codec_name, out_w, out_h
            );
        } else {
            self.dst_frame = Some(ffmpeg::util::frame::video::Video::new(
                Pixel::BGRA,
                out_w,
                out_h,
            ));
            self.scaler = None;
            info!(
                "Native FFmpeg encoder initialized: {} ({}x{}) with BGRA (no scaling)",
                codec_name, out_w, out_h
            );
        }

        self.last_input_res = (width, height);
        self.pending_packet_timestamps.clear();
        Ok(())
    }

    pub(super) fn drain_encoder_packets(&mut self, fallback_timestamp: i64) -> EncodeResult<()> {
        let mut packets_data: Vec<(Bytes, bool)> = Vec::with_capacity(8);

        if let Some(ref mut encoder) = self.encoder {
            let mut packet = ffmpeg::Packet::empty();
            while encoder.receive_packet(&mut packet).is_ok() {
                let data_slice = packet.data().unwrap_or(&[]);
                self.packet_buffer.extend_from_slice(data_slice);
                let data = self.packet_buffer.split_to(data_slice.len()).freeze();
                let packet_is_key = packet.is_key();
                packets_data.push((data, packet_is_key));
            }
        }

        let qpc_freq = crate::buffer::ring::qpc_frequency();
        let mut drained_count = 0usize;

        for (idx, (data, is_keyframe)) in packets_data.into_iter().enumerate() {
            let pts = self
                .pending_packet_timestamps
                .pop_front()
                .unwrap_or(fallback_timestamp);

            let hevc_nal: Option<u8> = if data.len() >= 5 && data[0..4] == [0x00, 0x00, 0x00, 0x01]
            {
                Some((data[4] >> 1) & 0x3f)
            } else if data.len() >= 4 && data[0..3] == [0x00, 0x00, 0x01] {
                Some((data[3] >> 1) & 0x3f)
            } else {
                None
            };

            if self.frame_count % 60 == 0 || is_keyframe || idx == 0 {
                let pts_ms = if qpc_freq > 0 {
                    pts * 1000 / qpc_freq
                } else {
                    0
                };
                let nal_name = match hevc_nal {
                    Some(32) => "VPS".to_string(),
                    Some(33) => "SPS".to_string(),
                    Some(34) => "PPS".to_string(),
                    Some(19) => "IDR_W_RADL".to_string(),
                    Some(20) => "IDR_N_LP".to_string(),
                    Some(1) => "TRAIL_R".to_string(),
                    Some(n) => format!("NAL{}", n),
                    None => "unknown".to_string(),
                };
                tracing::debug!(
                    "encoder packet: frame={} pts={}ms ({}B) nal={} keyframe={}",
                    self.frame_count,
                    pts_ms,
                    data.len(),
                    nal_name,
                    is_keyframe
                );
            }

            let mut encoded_packet =
                EncodedPacket::new(data, pts, pts, is_keyframe, StreamType::Video);

            if !self.config.use_native_resolution {
                encoded_packet.resolution = Some(self.config.resolution);
            }

            if self.packet_tx.send(encoded_packet).is_err() {
                break;
            }

            self.packet_count += 1;
            if self.frame_count <= super::WARMUP_FRAMES {
                self.warmup_packet_count += 1;
            }
            drained_count += 1;
        }

        if drained_count > 0
            && self.frame_count % 60 == 0
            && self.frame_count > super::WARMUP_FRAMES
        {
            let warmup_frames = super::WARMUP_FRAMES;
            let post_warmup_frames = self.frame_count - warmup_frames;
            let post_warmup_packets = self.packet_count - self.warmup_packet_count;
            let ratio = post_warmup_packets as f64 / post_warmup_frames.max(1) as f64;
            tracing::trace!(
                "encoder stats: frames={}, packets={}, ratio={:.2}",
                post_warmup_frames,
                post_warmup_packets,
                ratio
            );
            if ratio < 0.90 || ratio > 1.10 {
                tracing::warn!(
                    "Unexpected encoder packet/frame ratio {:.2} (frames={}, packets={})",
                    ratio,
                    post_warmup_frames,
                    post_warmup_packets
                );
            }
        }

        Ok(())
    }
}
