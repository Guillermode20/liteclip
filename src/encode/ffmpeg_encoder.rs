use super::{EncodedPacket, Encoder, EncoderConfig, StreamType};
use anyhow::{Context, Result};
use ffmpeg_next as ffmpeg;
use ffmpeg::format::Pixel;
use crossbeam::channel::{bounded, Receiver, Sender};
use tracing::{info};

pub struct FfmpegEncoder {
    config: EncoderConfig,
    encoder: Option<ffmpeg::encoder::Video>,
    packet_tx: Sender<EncodedPacket>,
    packet_rx: Receiver<EncodedPacket>,
    frame_count: i64,
    running: bool,
    // Software scaler for BGRA -> YUV conversion
    scaler: Option<ffmpeg::software::scaling::Context>,
}

// Safety: FfmpegEncoder is only used within a single dedicated encoder thread.
// The FFmpeg types it contains are not Send by default because they contain raw pointers,
// but they are safe to move to the dedicated thread before any processing starts.
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
        })
    }

    fn init_encoder(&mut self, width: u32, height: u32) -> Result<()> {
        let codec_name = self.config.ffmpeg_codec_name();
        let codec = ffmpeg::encoder::find_by_name(codec_name)
            .context(format!("Failed to find encoder: {}", codec_name))?;

        let mut encoder_ctx = ffmpeg::codec::context::Context::new_with_codec(codec);
        let mut encoder = encoder_ctx.encoder().video()
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
        
        // Bitrate and GOP settings
        let bitrate = (self.config.bitrate_mbps * 1_000_000) as usize;
        encoder.set_bit_rate(bitrate);
        encoder.set_max_bit_rate(bitrate);
        encoder.set_gop(self.config.keyframe_interval_frames());

        // Encoder specific options
        let mut options = ffmpeg::Dictionary::new();
        match self.config.encoder_type {
            crate::config::EncoderType::Software | crate::config::EncoderType::Auto => {
                options.set("preset", "ultrafast");
                options.set("tune", "zerolatency");
            }
            crate::config::EncoderType::Nvenc => {
                options.set("preset", "p1");
                options.set("tune", "ull");
                options.set("delay", "0");
            }
            crate::config::EncoderType::Amf => {
                options.set("usage", "lowlatency");
                options.set("quality", "speed");
            }
            crate::config::EncoderType::Qsv => {
                options.set("preset", "veryfast");
                options.set("look_ahead", "0");
            }
        }

        let encoder = encoder.open_with(options)
            .context("Failed to open encoder")?;

        self.encoder = Some(encoder);
        
        // Initialize scaler for BGRA -> YUV420P
        self.scaler = Some(ffmpeg::software::scaling::Context::get(
            Pixel::BGRA,
            width,
            height,
            Pixel::YUV420P,
            out_w,
            out_h,
            ffmpeg::software::scaling::flag::Flags::BILINEAR,
        ).context("Failed to create scaler context")?);

        info!("Native FFmpeg encoder initialized: {} ({}x{})", codec_name, out_w, out_h);
        Ok(())
    }

    fn receive_and_process_packets(&mut self) -> Result<()> {
        if let Some(ref mut encoder) = self.encoder {
            let mut packet = ffmpeg::Packet::empty();
            while encoder.receive_packet(&mut packet).is_ok() {
                let data = packet.data().unwrap_or(&[]).to_vec();
                let is_keyframe = packet.is_key();
                
                let pts = packet.pts().unwrap_or(self.frame_count);
                let dts = packet.dts().unwrap_or(pts);

                let mut encoded_packet = EncodedPacket::new(
                    data,
                    pts,
                    dts,
                    is_keyframe,
                    StreamType::Video,
                );
                
                if !self.config.use_native_resolution {
                    encoded_packet.resolution = Some(self.config.resolution);
                }

                if self.packet_tx.send(encoded_packet).is_err() {
                    break;
                }
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
        if self.encoder.is_none() {
            self.init_encoder(frame.resolution.0, frame.resolution.1)?;
        }

        let Some(ref mut encoder) = self.encoder else { return Ok(()); };
        let Some(ref mut scaler) = self.scaler else { return Ok(()); };

        // 1. Create source frame from raw BGRA data
        let mut src_frame = ffmpeg::util::frame::video::Video::new(Pixel::BGRA, frame.resolution.0, frame.resolution.1);
        src_frame.data_mut(0).copy_from_slice(&frame.bgra);

        // 2. Create destination frame (YUV420P)
        let mut dst_frame = ffmpeg::util::frame::video::Video::new(encoder.format(), encoder.width(), encoder.height());
        
        // 3. Scale/Convert BGRA -> YUV420P
        scaler.run(&src_frame, &mut dst_frame)?;
        
        // 4. Set timestamp
        dst_frame.set_pts(Some(frame.timestamp));

        // 5. Send frame to encoder
        encoder.send_frame(&dst_frame).context("Failed to send frame to encoder")?;

        // 6. Receive packets
        self.receive_and_process_packets()?;

        self.frame_count += 1;
        Ok(())
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>> {
        if let Some(ref mut encoder) = self.encoder {
            encoder.send_eof().ok();
            self.receive_and_process_packets().ok();
        }
        
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

