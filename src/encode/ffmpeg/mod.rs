pub mod amf;
pub mod context;
pub mod nvenc;
pub mod options;
pub mod qsv;
pub mod software;

use self::context::D3d11HardwareContext;
use super::{EncodedPacket, Encoder, EncoderConfig, StreamType};
use anyhow::{Context, Result};
use bytes::BytesMut;
use crossbeam::channel::{unbounded, Receiver, Sender};
use ffmpeg::color::{Primaries, Range, Space, TransferCharacteristic};
use ffmpeg_next as ffmpeg;
use std::collections::VecDeque;
use tracing::{info, warn};

pub struct FfmpegEncoder {
    config: EncoderConfig,
    encoder: Option<ffmpeg::encoder::Video>,
    packet_tx: Sender<EncodedPacket>,
    packet_rx: Receiver<EncodedPacket>,
    frame_count: i64,
    packet_count: i64,
    warmup_packet_count: i64,
    running: bool,
    scaler: Option<ffmpeg::software::scaling::Context>,
    src_frame: Option<ffmpeg::util::frame::video::Video>,
    dst_frame: Option<ffmpeg::util::frame::video::Video>,
    hw_context: Option<D3d11HardwareContext>,
    last_input_res: (u32, u32),
    pending_packet_timestamps: VecDeque<i64>,
    packet_buffer: BytesMut,
}

const WARMUP_FRAMES: i64 = 60;

unsafe impl Send for FfmpegEncoder {}

impl FfmpegEncoder {
    fn apply_bt709_encoder_metadata(encoder: &mut ffmpeg::encoder::video::Video) {
        encoder.set_colorspace(Space::BT709);
        encoder.set_color_range(Range::MPEG);
        unsafe {
            (*encoder.as_mut_ptr()).color_primaries = Primaries::BT709.into();
            (*encoder.as_mut_ptr()).color_trc = TransferCharacteristic::BT709.into();
        }
    }

    fn apply_bt709_frame_metadata(frame: &mut ffmpeg::util::frame::video::Video) {
        frame.set_color_space(Space::BT709);
        frame.set_color_range(Range::MPEG);
        frame.set_color_primaries(Primaries::BT709);
        frame.set_color_transfer_characteristic(TransferCharacteristic::BT709);
    }

    unsafe fn apply_bt709_raw_frame_metadata(frame: *mut ffmpeg::ffi::AVFrame) {
        (*frame).colorspace = Space::BT709.into();
        (*frame).color_range = Range::MPEG.into();
        (*frame).color_primaries = Primaries::BT709.into();
        (*frame).color_trc = TransferCharacteristic::BT709.into();
    }

    pub fn new(config: &EncoderConfig) -> Result<Self> {
        let (tx, rx) = unbounded();
        Ok(Self {
            config: config.clone(),
            encoder: None,
            packet_tx: tx,
            packet_rx: rx,
            frame_count: 0,
            packet_count: 0,
            warmup_packet_count: 0,
            running: false,
            scaler: None,
            src_frame: None,
            dst_frame: None,
            hw_context: None,
            last_input_res: (0, 0),
            pending_packet_timestamps: VecDeque::with_capacity(256),
            packet_buffer: BytesMut::with_capacity(1024 * 1024), // 1MB initial capacity
        })
    }

    pub(super) fn init_hardware_encoder(
        &mut self,
        gpu_frame: &crate::capture::D3d11Frame,
        width: u32,
        height: u32,
    ) -> Result<()> {
        match self.config.encoder_type {
            crate::config::EncoderType::Nvenc => {
                self.init_nvenc_hardware_encoder(gpu_frame, width, height)
            }
            crate::config::EncoderType::Amf => {
                self.init_amf_hardware_encoder(gpu_frame, width, height)
            }
            crate::config::EncoderType::Qsv => {
                self.init_qsv_hardware_encoder(gpu_frame, width, height)
            }
            _ => anyhow::bail!(
                "Hardware encoder initialization not supported for {:?}",
                self.config.encoder_type
            ),
        }
    }

    pub(super) fn encode_gpu_frame(
        &mut self,
        frame: &crate::capture::CapturedFrame,
        gpu_frame: &crate::capture::D3d11Frame,
        pts: i64,
        gop: i64,
    ) -> Result<()> {
        match self.config.encoder_type {
            crate::config::EncoderType::Nvenc => {
                self.encode_nvenc_gpu_frame(frame, gpu_frame, pts, gop)
            }
            crate::config::EncoderType::Amf => {
                self.encode_amf_gpu_frame(frame, gpu_frame, pts, gop)
            }
            crate::config::EncoderType::Qsv => {
                self.encode_qsv_gpu_frame(frame, gpu_frame, pts, gop)
            }
            _ => anyhow::bail!(
                "Hardware encoding not supported for {:?}",
                self.config.encoder_type
            ),
        }
    }

    fn supports_gpu_frames(&self) -> bool {
        self.config.supports_gpu_frame_transport()
    }

    fn next_encoder_pts(&self) -> i64 {
        self.frame_count
    }

    fn gpu_frame_matches_encoder(&self, gpu_frame: &crate::capture::D3d11Frame) -> bool {
        match self.config.gpu_texture_format() {
            Some(expected_format) => gpu_frame.format == expected_format,
            None => false,
        }
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
        let can_use_gpu = gpu_frame.is_some()
            && self.supports_gpu_frames()
            && self.gpu_frame_matches_encoder(gpu_frame.unwrap());
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
                        info!(
                            "GPU NV12 frames restored; reinitializing encoder for D3D11 transport"
                        );
                    } else {
                        info!(
                            "Initializing hardware encoder with D3D11 NV12 frames (GPU transport enabled)"
                        );
                    }
                    self.init_hardware_encoder(gpu_frame, frame.resolution.0, frame.resolution.1)?;
                }
            } else if gpu_frame.is_some() && self.supports_gpu_frames() {
                // GPU frame present but format does not match the selected encoder transport.
                if let Some(gpu_frame) = gpu_frame {
                    warn!(
                        "GPU frame format is {:?}, expected {:?} for encoder {:?}. Falling back to CPU path.",
                        gpu_frame.format,
                        self.config.gpu_texture_format(),
                        self.config.encoder_type
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

            Self::apply_bt709_frame_metadata(dst_frame);
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
