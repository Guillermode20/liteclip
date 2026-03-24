//! Software H.264 encoder via native `ffmpeg-next` libx264.

use crate::encode::{
    EncodeError, EncodeResult, EncodedPacket, Encoder, ResolvedEncoderConfig, StreamType,
};
use crossbeam::channel::{bounded, Receiver, Sender};
use ffmpeg_next as ffmpeg;
use ffmpeg_next::format::Pixel;
use std::thread::JoinHandle;
use tracing::{debug, info, warn};

pub struct CliPipeEncoder {
    config: ResolvedEncoderConfig,
    encoder: Option<ffmpeg::encoder::Video>,
    scaler: Option<ffmpeg::software::scaling::Context>,
    reader: Option<JoinHandle<()>>,
    packet_tx: Sender<EncodedPacket>,
    packet_rx: Receiver<EncodedPacket>,
    frame_count: i64,
    running: bool,
    input_frame: ffmpeg::util::frame::video::Video,
    output_frame: ffmpeg::util::frame::video::Video,
}

impl CliPipeEncoder {
    pub fn new(config: &ResolvedEncoderConfig) -> EncodeResult<Self> {
        // Use a bounded channel (256 packets ≈ ~4 s at 60 fps) so a stalled consumer
        // cannot grow the packet queue without bound.
        let (packet_tx, packet_rx) = bounded(256);
        let mut input_frame = ffmpeg::util::frame::video::Video::empty();
        let mut output_frame = ffmpeg::util::frame::video::Video::empty();
        input_frame.set_format(Pixel::BGRA);
        output_frame.set_format(Pixel::YUV420P);

        Ok(Self {
            config: config.clone(),
            encoder: None,
            scaler: None,
            reader: None,
            packet_tx,
            packet_rx,
            frame_count: 0,
            running: false,
            input_frame,
            output_frame,
        })
    }

    fn init_encoder(&mut self, width: u32, height: u32) -> EncodeResult<()> {
        let codec = ffmpeg::encoder::find_by_name("libx264")
            .ok_or_else(|| EncodeError::msg("libx264 codec not found in FFmpeg"))?;

        let mut context = ffmpeg::codec::context::Context::new_with_codec(codec);
        let mut encoder = context.encoder().video().map_err(|e| {
            EncodeError::msg(format!("failed to create video encoder context: {}", e))
        })?;

        encoder.set_width(width);
        encoder.set_height(height);
        encoder.set_format(Pixel::YUV420P);
        encoder.set_frame_rate(Some((self.config.framerate as i32, 1)));
        encoder.set_time_base((1, self.config.framerate as i32));
        encoder.set_bit_rate((self.config.bitrate_mbps as u32 * 1000) as u64);
        encoder.set_max_bit_rate((self.config.bitrate_mbps as u32 * 1000) as u64);
        encoder.set_gop(self.config.keyframe_interval_frames() as u32);

        let mut options = ffmpeg::Dictionary::new();
        options.set("preset", "ultrafast");
        options.set("tune", "zerolatency");
        options.set("bf", "0");
        options.set("nal-hrd", "none");

        encoder
            .open_with(options)
            .map_err(|e| EncodeError::msg(format!("failed to open encoder: {}", e)))?;

        let scaler = ffmpeg::software::scaling::Context::get(
            Pixel::BGRA,
            width,
            height,
            Pixel::YUV420P,
            width,
            height,
            ffmpeg::software::scaling::Flags::BILINEAR,
        )
        .map_err(|e| EncodeError::msg(format!("failed to create scaler: {}", e)))?;

        self.encoder = Some(encoder);
        self.scaler = Some(scaler);
        self.input_frame.set_width(width);
        self.input_frame.set_height(height);
        self.output_frame.set_width(width);
        self.output_frame.set_height(height);
        self.output_frame.set_format(Pixel::YUV420P);
        if self.output_frame.alloc() != 0 {
            return Err(EncodeError::msg("failed to allocate output frame"));
        }

        self.running = true;
        info!(
            "Native encoder started (libx264 {}x{} @ {} fps)",
            width, height, self.config.framerate
        );
        Ok(())
    }
}

impl Encoder for CliPipeEncoder {
    fn init(&mut self, config: &ResolvedEncoderConfig) -> EncodeResult<()> {
        self.config = config.clone();
        let (w, h) = if config.use_native_resolution {
            (config.resolution.0, config.resolution.1)
        } else {
            config.resolution
        };
        if w == 0 || h == 0 {
            return Err(EncodeError::msg("invalid resolution for encoder"));
        }
        self.init_encoder(w, h)
    }

    fn encode_frame(&mut self, frame: &crate::media::CapturedFrame) -> EncodeResult<()> {
        if frame.bgra.is_empty() {
            return Err(EncodeError::msg(
                "encoder requires CPU BGRA pixels; enable CPU readback in settings",
            ));
        }

        let (w, h) = frame.resolution;
        let expected = (w as usize) * (h as usize) * 4;
        if frame.bgra.len() < expected {
            return Err(EncodeError::msg(format!(
                "BGRA buffer too small: got {} need {}",
                frame.bgra.len(),
                expected
            )));
        }

        let encoder = self
            .encoder
            .as_mut()
            .ok_or_else(|| EncodeError::msg("encoder not initialized"))?;
        let scaler = self
            .scaler
            .as_mut()
            .ok_or_else(|| EncodeError::msg("scaler not initialized"))?;

        // Set input frame data
        unsafe {
            self.input_frame.set_data(&[&frame.bgra[..expected]]);
            self.input_frame.set_linesize(&[(w * 4) as i32]);
        }

        // Scale BGRA to YUV420P
        scaler
            .run(&self.input_frame, &mut self.output_frame)
            .map_err(|e| EncodeError::msg(format!("scaling failed: {}", e)))?;

        // Set PTS
        self.output_frame.set_pts(Some(self.frame_count));

        // Send frame to encoder
        encoder
            .send_frame(&self.output_frame)
            .map_err(|e| EncodeError::msg(format!("encoder send_frame failed: {}", e)))?;

        // Receive and emit any encoded packets
        let mut pkt = ffmpeg::packet::Packet::empty();
        while encoder.receive_packet(&mut pkt).is_ok() {
            let is_key = pkt.is_key();
            let data = pkt.data().unwrap_or_default().to_vec();
            let encoded = EncodedPacket::new(
                data,
                self.frame_count,
                self.frame_count,
                is_key,
                StreamType::Video,
            );
            let _ = self.packet_tx.send(encoded);
        }

        self.frame_count += 1;
        Ok(())
    }

    fn flush(&mut self) -> EncodeResult<Vec<EncodedPacket>> {
        let mut out = Vec::new();

        if let Some(encoder) = self.encoder.as_mut() {
            // Flush encoder by sending null frame
            encoder
                .send_frame(None)
                .map_err(|e| EncodeError::msg(format!("encoder flush failed: {}", e)))?;

            // Receive remaining packets
            let mut pkt = ffmpeg::packet::Packet::empty();
            while encoder.receive_packet(&mut pkt).is_ok() {
                let is_key = pkt.is_key();
                let data = pkt.data().unwrap_or_default().to_vec();
                let encoded = EncodedPacket::new(
                    data,
                    self.frame_count,
                    self.frame_count,
                    is_key,
                    StreamType::Video,
                );
                out.push(encoded);
            }
        }

        while let Ok(p) = self.packet_rx.try_recv() {
            out.push(p);
        }

        self.running = false;
        Ok(out)
    }

    fn packet_rx(&self) -> Receiver<EncodedPacket> {
        self.packet_rx.clone()
    }

    fn is_running(&self) -> bool {
        self.running
    }
}
