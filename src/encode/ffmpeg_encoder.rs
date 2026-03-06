use super::{EncodedPacket, Encoder, EncoderConfig, StreamType};
use crate::config::{EncoderType, QualityPreset, RateControl};
use anyhow::{Context, Result};
use crossbeam::channel::{bounded, Receiver, Sender};
use ffmpeg::format::Pixel;
use ffmpeg_next as ffmpeg;
use std::collections::VecDeque;
use tracing::info;

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
            last_input_res: (0, 0),
            pending_packet_timestamps: VecDeque::with_capacity(256),
        })
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
        encoder.set_format(Pixel::YUV420P);
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
        self.src_frame = Some(ffmpeg::util::frame::video::Video::new(
            Pixel::BGRA,
            width,
            height,
        ));
        self.dst_frame = Some(ffmpeg::util::frame::video::Video::new(
            Pixel::YUV420P,
            out_w,
            out_h,
        ));
        self.scaler = Some(
            ffmpeg::software::scaling::Context::get(
                Pixel::BGRA,
                width,
                height,
                Pixel::YUV420P,
                out_w,
                out_h,
                ffmpeg::software::scaling::flag::Flags::FAST_BILINEAR,
            )
            .context("Failed to create scaler context")?,
        );

        self.last_input_res = (width, height);
        self.pending_packet_timestamps.clear();

        info!(
            "Native FFmpeg encoder initialized: {} ({}x{})",
            codec_name, out_w, out_h
        );
        Ok(())
    }

    fn drain_encoder_packets(&mut self, fallback_timestamp: i64) -> Result<()> {
        let mut drained_packets = Vec::new();

        if let Some(ref mut encoder) = self.encoder {
            let mut packet = ffmpeg::Packet::empty();
            while encoder.receive_packet(&mut packet).is_ok() {
                drained_packets.push((
                    packet.data().unwrap_or(&[]).to_vec(),
                    packet.is_key(),
                    packet.pts().unwrap_or(0),
                ));
                packet = ffmpeg::Packet::empty();
            }
        }

        for (data, packet_is_key, _packet_pts) in drained_packets {
            // HEVC uses annex-b format
            let normalized_data = Self::convert_length_prefixed_to_annex_b(&data);
            let inspection_data = normalized_data.as_deref().unwrap_or(data.as_slice());
            let is_keyframe = Self::detect_keyframe(inspection_data, packet_is_key);
            let pts = self.dequeue_packet_timestamp(fallback_timestamp);

            // only log occasionally to avoid spam; keyframes always get logged
            if self.frame_count % 60 == 0 || is_keyframe {
                // use debug level and a simpler message so the output is less obtuse
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
        if self.encoder.is_none() || self.last_input_res != (frame.resolution.0, frame.resolution.1)
        {
            self.init_encoder(frame.resolution.0, frame.resolution.1)?;
        }

        let encoder_pts = self.next_encoder_pts();
        let gop = self.config.keyframe_interval_frames() as i64;
        self.pending_packet_timestamps.push_back(frame.timestamp);
        if self.pending_packet_timestamps.len() > 512 {
            self.pending_packet_timestamps.pop_front();
        }

        {
            let Some(ref mut encoder) = self.encoder else {
                return Ok(());
            };
            let Some(ref mut scaler) = self.scaler else {
                return Ok(());
            };
            let Some(ref mut src_frame) = self.src_frame else {
                return Ok(());
            };
            let Some(ref mut dst_frame) = self.dst_frame else {
                return Ok(());
            };

            src_frame.data_mut(0).copy_from_slice(&frame.bgra);
            scaler.run(src_frame, dst_frame)?;

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