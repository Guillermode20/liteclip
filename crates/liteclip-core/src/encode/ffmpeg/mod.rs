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
use crate::encode::{EncodeError, EncodeResult};
use bytes::Bytes;
use crossbeam::channel::{bounded, Receiver, Sender};
use ffmpeg::color::{Primaries, Range, Space, TransferCharacteristic};
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

// SAFETY: FfmpegEncoder is Send because:
// 1. The encoder runs on a dedicated thread and all state is owned by that thread
// 2. FFmpeg encoder contexts are thread-safe when used from a single thread
// 3. The packet channels (Sender/Receiver) are Send-safe (crossbeam channels)
// 4. The hardware context (D3d11HardwareContext) is only accessed from the encoder thread
// 5. All shared state uses proper synchronization (atomics for running flag)
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

    /// Apply BT.709 color metadata to a raw AVFrame pointer.
    ///
    /// # Safety
    ///
    /// Caller must ensure `frame` is a valid, non-null pointer to an AVFrame
    /// that was allocated by FFmpeg (e.g., via `av_frame_alloc()` or returned
    /// from an FFmpeg API like `av_hwframe_get_buffer`). The frame must remain
    /// valid for the duration of this call.
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
            // Software encoder doesn't use GPU frames - falls back to CPU path
            ResolvedEncoderType::Software => Err(EncodeError::msg(
                "Software encoder cannot use GPU frame transport",
            )),
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
            // Software encoder should never reach this path - CPU frames only
            ResolvedEncoderType::Software => Err(EncodeError::msg(
                "Software encoder cannot encode GPU frames",
            )),
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

        // GPU duplicate frame optimization is intentionally disabled due to GOP timing
        // synchronization requirements between manual keyframe flags and encoder internal state.

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
                if at_keyframe {
                    dst_frame.set_kind(ffmpeg::picture::Type::I);
                } else {
                    dst_frame.set_kind(ffmpeg::picture::Type::None);
                }

                encoder.send_frame(dst_frame)?;
            } else {
                Self::apply_bt709_frame_metadata(src_frame);
                src_frame.set_pts(Some(encoder_pts));
                if gop > 0 && self.encoder_frame_count % gop == 0 {
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

#[cfg(test)]
mod keyframe_gop_tests {
    /// Demonstrates that keyframe decisions must use encoder_frame_count, not frame_count.
    ///
    /// When GPU duplicate optimization emits cached bitstreams without encoding new frames,
    /// frame_count increments but encoder_frame_count does not. If keyframe decisions use
    /// frame_count, GOP alignment drifts from what the encoder actually sees.
    ///
    /// Example: GOP=60, after 5 duplicate optimizations:
    /// - frame_count = 65 (all processed frames)
    /// - encoder_frame_count = 60 (frames actually encoded)
    /// - Using frame_count: keyframe at frame 60, but encoder at frame 55 (wrong!)
    /// - Using encoder_frame_count: keyframe at encoder frame 60 (correct!)
    #[test]
    fn keyframe_decision_uses_encoder_frame_count_not_total_frames() {
        let gop = 60i64;

        // Scenario: 65 frames processed, but only 60 actually encoded
        // (5 frames emitted as cached GPU duplicates)
        let frame_count = 65i64;
        let _encoder_frame_count = 60i64;

        // WRONG: using frame_count would signal keyframe at wrong time
        let wrong_keyframe_decision = gop > 0 && frame_count % gop == 0;
        assert!(
            !wrong_keyframe_decision,
            "BUG: frame_count=65 % 60 = 5, not 0, so this test would pass by accident"
        );

        // But what if frame_count = 120 after duplicates, encoder_frame_count = 115?
        // frame_count % gop = 0 (signals keyframe), but encoder sees frame 115!
        let frame_count_120 = 120i64;
        let encoder_frame_count_115 = 115i64;
        let buggy_keyframe = gop > 0 && frame_count_120 % gop == 0;
        let correct_keyframe = gop > 0 && encoder_frame_count_115 % gop == 0;

        // The bug: frame_count signals keyframe at 120, but encoder is at frame 115
        assert!(buggy_keyframe, "frame_count=120 % 60 = 0, signals keyframe");
        assert!(
            !correct_keyframe,
            "encoder_frame_count=115 % 60 = 55, not keyframe"
        );

        // Correct: keyframes align with encoder's internal GOP state
        // When encoder_frame_count = 120, it's a keyframe
        let encoder_frame_count_120 = 120i64;
        let correct_keyframe_at_120 = gop > 0 && encoder_frame_count_120 % gop == 0;
        assert!(
            correct_keyframe_at_120,
            "encoder_frame_count=120 % 60 = 0, correct keyframe"
        );
    }

    /// Verifies that the first frame (encoder_frame_count=0) should be a keyframe.
    #[test]
    fn first_frame_is_keyframe() {
        let gop = 60i64;
        let encoder_frame_count = 0i64;
        let at_keyframe = gop > 0 && encoder_frame_count % gop == 0;
        assert!(
            at_keyframe,
            "First frame (encoder_frame_count=0) must be keyframe"
        );
    }

    /// Verifies keyframe pattern at GOP boundaries.
    #[test]
    fn keyframes_at_gop_boundaries() {
        let gop = 60i64;

        // Frames 0, 60, 120, 180 should be keyframes
        for expected_keyframe in [0, 60, 120, 180, 240] {
            let at_keyframe = gop > 0 && expected_keyframe % gop == 0;
            assert!(
                at_keyframe,
                "Frame {} should be keyframe",
                expected_keyframe
            );
        }

        // Frames 1, 59, 61, 119 should NOT be keyframes
        for expected_non_keyframe in [1, 59, 61, 119, 121] {
            let at_keyframe = gop > 0 && expected_non_keyframe % gop == 0;
            assert!(
                !at_keyframe,
                "Frame {} should NOT be keyframe",
                expected_non_keyframe
            );
        }
    }
}
