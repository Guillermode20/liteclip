//! FFmpeg-backed encoder implementation (`FfmpegEncoder`).
//!
//! # Contributing: hardware encoders (NVENC, QSV, AMF)
//!
//! Vendor-specific **encoding** lives in dedicated files. **AMF** ([`amf`](crate::encode::ffmpeg::amf))
//! is the path most maintainers can run locally; **NVENC** ([`nvenc`](crate::encode::ffmpeg::nvenc)) and
//! **QSV** ([`qsv`](crate::encode::ffmpeg::qsv)) need NVIDIA or Intel hardware (and matching FFmpeg) to verify.
//!
//! When you add or change a hardware encoder (new codec name, options, probe behavior, or GPU frame path),
//! keep these locations consistent:
//!
//! - **Init + per-frame GPU path:** [`nvenc`](crate::encode::ffmpeg::nvenc), [`qsv`](crate::encode::ffmpeg::qsv), [`amf`](crate::encode::ffmpeg::amf)
//! - **Shared FFmpeg options:** [`options`](crate::encode::ffmpeg::options)
//! - **Dispatch:** `init_hardware_encoder` / `encode_gpu_frame` in this module
//! - **Codec name, GPU transport flags:** [`ResolvedEncoderConfig`](crate::encode::encoder_mod::ResolvedEncoderConfig)
//!   in `encoder_mod/types.rs` (`ffmpeg_codec_name`, `supports_gpu_frame_transport`, `gpu_texture_format`)
//! - **Availability probe + auto-detect order:** `encoder_mod/functions.rs` (`probe_encoder_available`,
//!   `detect_hardware_encoder`)
//! - **User-facing enum + labels:** `config/config_mod/types.rs` (`EncoderType`), `gui/settings.rs` (encoder picker)
//!
//! **FFmpeg / runtime:** Encoders must exist in the FFmpeg build linked at runtime. NVENC requires an NVIDIA
//! GPU and driver with NVENC support. QSV requires Intel graphics with a working oneVPL/Media Stack stack and
//! FFmpeg built with QSV. Probe failures often mean a generic FFmpeg build without that encoder enabled.
//!
//! **Manual verification:** Force the encoder in settings (not Auto), record a short clip, and confirm logs
//! show hardware/D3D11 transport (no unexpected fallback to CPU path warnings for GPU-capable configs).
//! For PRs, include GPU model, driver/FFmpeg build notes, and relevant log lines—maintainers may not have
//! every vendor’s hardware.
//!
//! **Decode / gallery:** Clip preview decode uses generic D3D11VA in `gui/gallery/decode_pipeline.rs`, not
//! per-vendor NVENC/QSV **decode** paths. Encoding issues are unlikely to be fixed there.
//!
//! **CI:** Release builds use standard Windows runners; there is no GPU matrix. Vendor encoder changes rely
//! on contributor or reviewer manual testing.

pub mod amf;
pub mod context;
pub mod nvenc;
pub mod options;
pub mod qsv;
pub mod software;

use self::context::D3d11HardwareContext;
use super::{EncodedPacket, Encoder, ResolvedEncoderConfig, ResolvedEncoderType, StreamType};
use crate::encode::EncodeResult;
use crossbeam::channel::{bounded, Receiver, Sender};
use ffmpeg::color::{Primaries, Range, Space, TransferCharacteristic};
use bytes::Bytes;
use ffmpeg_next as ffmpeg;
use std::collections::VecDeque;
#[cfg(windows)]
use std::sync::Arc;
use tracing::{info, warn};

pub struct FfmpegEncoder {
    config: ResolvedEncoderConfig,
    encoder: Option<ffmpeg::encoder::Video>,
    packet_tx: Sender<EncodedPacket>,
    packet_rx: Receiver<EncodedPacket>,
    /// Total frames processed (including duplicates emitted without encoding).
    frame_count: i64,
    /// Frames actually sent to the encoder (used for keyframe/GOP decisions).
    /// This diverges from frame_count when GPU duplicate optimization emits
    /// cached bitstreams instead of encoding new frames.
    encoder_frame_count: i64,
    packet_count: i64,
    warmup_packet_count: i64,
    running: bool,
    scaler: Option<ffmpeg::software::scaling::Context>,
    src_frame: Option<ffmpeg::util::frame::video::Video>,
    dst_frame: Option<ffmpeg::util::frame::video::Video>,
    hw_context: Option<D3d11HardwareContext>,
    last_input_res: (u32, u32),
    pending_packet_timestamps: VecDeque<i64>,
    /// [`Arc::as_ptr`] of the last captured GPU frame (DXGI duplicate / static scene fast path).
    last_gpu_frame_arc_ptr: Option<usize>,
    /// Bitstream template from the last real encode: reused when the GPU frame is unchanged.
    last_duplicate_template: Vec<(Bytes, bool)>,
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

    pub fn new(config: &ResolvedEncoderConfig) -> EncodeResult<Self> {
        let (tx, rx) = bounded(1024);
        Ok(Self {
            config: config.clone(),
            encoder: None,
            packet_tx: tx,
            packet_rx: rx,
            frame_count: 0,
            encoder_frame_count: 0,
            packet_count: 0,
            warmup_packet_count: 0,
            running: false,
            scaler: None,
            src_frame: None,
            dst_frame: None,
            hw_context: None,
            last_input_res: (0, 0),
            pending_packet_timestamps: VecDeque::with_capacity(256),
            last_gpu_frame_arc_ptr: None,
            last_duplicate_template: Vec::new(),
        })
    }

    fn clear_gpu_duplicate_state(&mut self) {
        self.last_gpu_frame_arc_ptr = None;
        self.last_duplicate_template.clear();
    }

    /// Re-emit the last encoded video bitstream with a new wall-clock PTS (static / duplicate DXGI frame).
    fn emit_duplicate_video_packets(&mut self, timestamp: i64) -> EncodeResult<()> {
        for (data, is_keyframe) in &self.last_duplicate_template {
            let mut encoded_packet = EncodedPacket::new(
                data.clone(),
                timestamp,
                timestamp,
                *is_keyframe,
                StreamType::Video,
            );
            if !self.config.use_native_resolution {
                encoded_packet.resolution = Some(self.config.resolution);
            }
            if self.packet_tx.send(encoded_packet).is_err() {
                break;
            }
            self.packet_count += 1;
        }
        Ok(())
    }

    pub(super) fn init_hardware_encoder(
        &mut self,
        gpu_frame: &crate::media::D3d11Frame,
        width: u32,
        height: u32,
    ) -> EncodeResult<()> {
        match self.config.encoder_type {
            ResolvedEncoderType::Nvenc => {
                self.init_nvenc_hardware_encoder(gpu_frame, width, height)
            }
            ResolvedEncoderType::Amf => self.init_amf_hardware_encoder(gpu_frame, width, height),
            ResolvedEncoderType::Qsv => self.init_qsv_hardware_encoder(gpu_frame, width, height),
        }
    }

    pub(super) fn encode_gpu_frame(
        &mut self,
        frame: &crate::media::CapturedFrame,
        gpu_frame: &crate::media::D3d11Frame,
        pts: i64,
        gop: i64,
    ) -> EncodeResult<()> {
        match self.config.encoder_type {
            ResolvedEncoderType::Nvenc => self.encode_nvenc_gpu_frame(frame, gpu_frame, pts, gop),
            ResolvedEncoderType::Amf => self.encode_amf_gpu_frame(frame, gpu_frame, pts, gop),
            ResolvedEncoderType::Qsv => self.encode_qsv_gpu_frame(frame, gpu_frame, pts, gop),
        }
    }

    fn supports_gpu_frames(&self) -> bool {
        self.config.supports_gpu_frame_transport()
    }

    fn next_encoder_pts(&self) -> i64 {
        self.encoder_frame_count
    }

    fn gpu_frame_matches_encoder(&self, gpu_frame: &crate::media::D3d11Frame) -> bool {
        match self.config.gpu_texture_format() {
            Some(expected_format) => gpu_frame.format == expected_format,
            None => false,
        }
    }
}

impl Encoder for FfmpegEncoder {
    fn init(&mut self, _config: &ResolvedEncoderConfig) -> EncodeResult<()> {
        self.running = true;
        Ok(())
    }

    fn encode_frame(&mut self, frame: &crate::media::CapturedFrame) -> EncodeResult<()> {
        let gpu_frame = frame.d3d11.as_deref();

        // Check if we can use GPU frame transport
        let can_use_gpu = match gpu_frame {
            Some(gf) => self.supports_gpu_frames() && self.gpu_frame_matches_encoder(gf),
            None => false,
        };
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

        if !can_use_gpu {
            self.clear_gpu_duplicate_state();
        }

        let gop = self.config.keyframe_interval_frames() as i64;
        // Keyframe decision based on encoder_frame_count - this matches what the encoder
        // actually sees, keeping in sync with the encoder's internal GOP state.
        let at_keyframe = gop > 0 && self.encoder_frame_count % gop == 0;

        // GPU duplicate frame optimization DISABLED.
        // 
        // The optimization conflicts with the encoder's internal GOP management. When duplicates
        // are emitted, frame_count advances but encoder_frame_count doesn't, causing keyframe
        // decisions to diverge from the encoder's actual frame count. This results in missing
        // keyframes and corrupted video output.
        //
        // The encoder has its own GOP setting (set_gop) that expects keyframes at regular
        // intervals based on frames it receives. We also manually set key_frame=1 for explicit
        // control. These two mechanisms must stay synchronized, which requires encoder_frame_count
        // to be used for all keyframe decisions.
        //
        // Re-enabling this optimization would require either:
        // 1. Letting the encoder handle all GOP decisions (remove manual keyframe setting)
        // 2. Or tracking duplicate state in a way that doesn't affect GOP timing
        #[cfg(windows)]
        {
            if false && can_use_gpu && !at_keyframe {
                if let Some(d3d) = &frame.d3d11 {
                    let ptr = Arc::as_ptr(d3d) as usize;
                    if self.encoder_frame_count >= WARMUP_FRAMES
                        && self.last_gpu_frame_arc_ptr == Some(ptr)
                        && !self.last_duplicate_template.is_empty()
                        && self
                            .last_duplicate_template
                            .iter()
                            .all(|(_, is_key)| !*is_key)
                    {
                        self.emit_duplicate_video_packets(frame.timestamp)?;
                        self.frame_count += 1;
                        // Note: encoder_frame_count is NOT incremented here since we didn't encode
                        self.last_gpu_frame_arc_ptr = Some(ptr);
                        return Ok(());
                    }
                }
            }
        }

        self.last_duplicate_template.clear();

        let encoder_pts = self.next_encoder_pts();
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

            // When no scaler is needed, send the populated source frame directly to avoid
            // a second full-frame memcpy into `dst_frame`.
            if let Some(ref mut scaler) = self.scaler {
                scaler.run(src_frame, dst_frame)?;
                Self::apply_bt709_frame_metadata(dst_frame);
                dst_frame.set_pts(Some(encoder_pts));
                if gop > 0 && self.frame_count % gop == 0 {
                    dst_frame.set_kind(ffmpeg::picture::Type::I);
                } else {
                    dst_frame.set_kind(ffmpeg::picture::Type::None);
                }

                encoder.send_frame(dst_frame)?;
            } else {
                Self::apply_bt709_frame_metadata(src_frame);
                src_frame.set_pts(Some(encoder_pts));
                if gop > 0 && self.frame_count % gop == 0 {
                    src_frame.set_kind(ffmpeg::picture::Type::I);
                } else {
                    src_frame.set_kind(ffmpeg::picture::Type::None);
                }

                encoder.send_frame(src_frame)?;
            }
        }

        self.drain_encoder_packets(frame.timestamp)?;
        self.frame_count += 1;
        self.encoder_frame_count += 1;
        #[cfg(windows)]
        if can_use_gpu {
            if let Some(d3d) = &frame.d3d11 {
                self.last_gpu_frame_arc_ptr = Some(Arc::as_ptr(d3d) as usize);
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> EncodeResult<Vec<EncodedPacket>> {
        if let Some(ref mut encoder) = self.encoder {
            encoder.send_eof().ok();
        }

        self.drain_encoder_packets(0)?;

        let mut packets = Vec::new();
        while let Ok(packet) = self.packet_rx.try_recv() {
            packets.push(packet);
        }

        self.clear_gpu_duplicate_state();
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

#[cfg(test)]
mod gpu_duplicate_template_tests {
    use bytes::Bytes;

    #[test]
    fn template_eligible_for_reuse_only_when_no_keyframe_packet() {
        let ok = vec![(Bytes::from_static(b"x"), false)];
        assert!(ok.iter().all(|(_, k)| !*k));
        let bad = vec![(Bytes::from_static(b"x"), true)];
        assert!(!bad.iter().all(|(_, k)| !*k));
    }
}
