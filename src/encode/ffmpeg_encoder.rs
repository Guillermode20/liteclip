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

    fn bitrate_kbps(&self) -> u32 {
        self.config.bitrate_mbps.max(1) * 1000
    }

    fn peak_bitrate_kbps(&self) -> u32 {
        match self.config.rate_control {
            RateControl::Cbr => self.bitrate_kbps(),
            RateControl::Vbr | RateControl::Cq => self.bitrate_kbps().saturating_mul(2),
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

    fn software_preset(&self) -> &'static str {
        match self.config.quality_preset {
            QualityPreset::Performance => "veryfast",
            QualityPreset::Balanced => "medium",
            QualityPreset::Quality => "slow",
        }
    }

    fn software_tune(&self) -> &'static str {
        "zerolatency"
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

    fn software_target_bitrate_param(&self) -> String {
        format!("{}k", self.bitrate_kbps())
    }

    fn software_peak_bitrate_param(&self) -> String {
        format!("{}k", self.peak_bitrate_kbps())
    }

    fn software_bufsize_param(&self) -> String {
        format!("{}k", self.bitrate_kbps())
    }

    fn software_h264_params(&self, keyint: u32) -> String {
        let bitrate_kbps = self.bitrate_kbps();
        let peak_bitrate_kbps = self.peak_bitrate_kbps();

        match self.config.rate_control {
            RateControl::Cbr => format!(
                "force-cfr=1:nal-hrd=cbr:scenecut=0:keyint={}:min-keyint={}:vbv-maxrate={}:vbv-bufsize={}",
                keyint, keyint, bitrate_kbps, bitrate_kbps
            ),
            RateControl::Vbr => format!(
                "force-cfr=1:scenecut=0:keyint={}:min-keyint={}:vbv-maxrate={}:vbv-bufsize={}",
                keyint, keyint, peak_bitrate_kbps, bitrate_kbps
            ),
            RateControl::Cq => format!(
                "force-cfr=1:scenecut=0:keyint={}:min-keyint={}:crf={}:vbv-maxrate={}:vbv-bufsize={}",
                keyint,
                keyint,
                self.cq_value(),
                peak_bitrate_kbps,
                bitrate_kbps
            ),
        }
    }

    fn software_h265_params(&self, keyint: u32) -> String {
        let bitrate_kbps = self.bitrate_kbps();
        let peak_bitrate_kbps = self.peak_bitrate_kbps();
        let low_memory_params = "pools=none:frame-threads=1:ref=1";

        match self.config.rate_control {
            RateControl::Cbr => format!(
                "repeat-headers=1:aud=1:open-gop=0:scenecut=0:keyint={}:min-keyint={}:bitrate={}:vbv-maxrate={}:vbv-bufsize={}:strict-cbr=1:{}",
                keyint, keyint, bitrate_kbps, bitrate_kbps, bitrate_kbps, low_memory_params
            ),
            RateControl::Vbr => format!(
                "repeat-headers=1:aud=1:open-gop=0:scenecut=0:keyint={}:min-keyint={}:bitrate={}:vbv-maxrate={}:vbv-bufsize={}:{}",
                keyint, keyint, bitrate_kbps, peak_bitrate_kbps, bitrate_kbps, low_memory_params
            ),
            RateControl::Cq => format!(
                "repeat-headers=1:aud=1:open-gop=0:scenecut=0:keyint={}:min-keyint={}:crf={}:vbv-maxrate={}:vbv-bufsize={}:{}",
                keyint,
                keyint,
                self.cq_value(),
                peak_bitrate_kbps,
                bitrate_kbps,
                low_memory_params
            ),
        }
    }

    fn dequeue_packet_timestamp(&mut self, fallback: i64) -> i64 {
        self.pending_packet_timestamps
            .pop_front()
            .unwrap_or(fallback)
    }

    fn detect_keyframe(data: &[u8], packet_is_key: bool) -> bool {
        if packet_is_key || data.is_empty() {
            return packet_is_key;
        }

        let mut i = 0usize;
        while i + 4 < data.len() && i < 100 {
            if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 0 && data[i + 3] == 1 {
                let nal_byte = data[i + 4];
                let h264_type = nal_byte & 0x1f;
                let hevc_type = (nal_byte >> 1) & 0x3f;
                if h264_type == 7
                    || h264_type == 5
                    || hevc_type == 32
                    || hevc_type == 33
                    || hevc_type == 19
                    || hevc_type == 20
                {
                    return true;
                }
                i += 4;
            } else if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
                let nal_byte = data[i + 3];
                let h264_type = nal_byte & 0x1f;
                let hevc_type = (nal_byte >> 1) & 0x3f;
                if h264_type == 7
                    || h264_type == 5
                    || hevc_type == 32
                    || hevc_type == 33
                    || hevc_type == 19
                    || hevc_type == 20
                {
                    return true;
                }
                i += 3;
            } else {
                i += 1;
            }
        }

        false
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
            EncoderType::Software | EncoderType::Auto => {
                let keyint = self.config.keyframe_interval_frames().max(1);
                let bitrate_param = self.software_target_bitrate_param();
                let peak_bitrate_param = self.software_peak_bitrate_param();
                let bufsize_param = self.software_bufsize_param();

                options.set("preset", self.software_preset());
                options.set("tune", self.software_tune());
                options.set("b", &bitrate_param);
                options.set("maxrate", &peak_bitrate_param);
                options.set("bufsize", &bufsize_param);
                options.set(
                    "threads",
                    match self.config.codec {
                        crate::config::Codec::H265 => "2",
                        crate::config::Codec::Av1 => "2",
                        crate::config::Codec::H264 => "0",
                    },
                );

                match self.config.codec {
                    crate::config::Codec::H264 => {
                        options.set("x264-params", &self.software_h264_params(keyint));
                        options.set("qmin", "18");
                        options.set("rc-lookahead", "0");
                        if matches!(self.config.rate_control, RateControl::Cbr) {
                            options.set("minrate", &bitrate_param);
                        }
                        if matches!(self.config.rate_control, RateControl::Cq) {
                            options.set("crf", &self.cq_value().to_string());
                        }
                    }
                    crate::config::Codec::H265 => {
                        options.set("x265-params", &self.software_h265_params(keyint));
                        if matches!(self.config.rate_control, RateControl::Cbr) {
                            options.set("minrate", &bitrate_param);
                        }
                        if matches!(self.config.rate_control, RateControl::Cq) {
                            options.set("crf", &self.cq_value().to_string());
                        }
                    }
                    crate::config::Codec::Av1 => {
                        options.set(
                            "usage",
                            match self.config.quality_preset {
                                QualityPreset::Performance => "realtime",
                                QualityPreset::Balanced | QualityPreset::Quality => "good",
                            },
                        );
                        options.set("lag-in-frames", "0");
                        options.set(
                            "cpu-used",
                            match self.config.quality_preset {
                                QualityPreset::Performance => "8",
                                QualityPreset::Balanced => "6",
                                QualityPreset::Quality => "4",
                            },
                        );
                        if matches!(self.config.rate_control, RateControl::Cq) {
                            options.set("crf", &self.cq_value().to_string());
                        }
                    }
                }
            }
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

                options.set("usage", "lowlatency");
                options.set("quality", self.amf_quality());
                options.set("rc", self.amf_rc_mode());
                options.set("aud", "1");
                options.set("bf", "0");
                options.set("header_insertion_mode", "idr");
                options.set("gops_per_idr", "1");
                options.set("pa_adaptive_mini_gop", "0");
                options.set("preanalysis", "1");
                options.set("vbaq", "1");
                options.set("rc_lookahead", "8");
                options.set("max_qp_delta", "4");
                options.set("filler_data", "0");
                options.set("me_half_pel", "1");
                options.set("me_quarter_pel", "1");
                options.set("high_motion_quality_boost_enable", "1");
                options.set("min_qp_i", "18");
                options.set("max_qp_i", "51");
                options.set("min_qp_p", "20");
                options.set("max_qp_p", "51");

                if matches!(self.config.codec, crate::config::Codec::H264) {
                    options.set("coder", "cabac");
                }
                if matches!(self.config.codec, crate::config::Codec::H265) {
                    options.set("profile_tier", "high");
                }

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

        for (data, packet_is_key, packet_pts) in drained_packets {
            let is_keyframe = Self::detect_keyframe(&data, packet_is_key);
            let pts = self.dequeue_packet_timestamp(fallback_timestamp);

            if self.frame_count % 60 == 0 || is_keyframe {
                tracing::info!(
                    "Packet received: size={}, pts={:?}, is_key={}",
                    data.len(),
                    packet_pts,
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
