//! Software Encoder
//!
//! Parallel JPEG compression pipeline. Uses a thread pool of workers to
//! encode BGRA frames into MJPEG packets for the ring buffer.
//! Each worker pre-allocates its RGB conversion buffer to avoid per-frame
//! allocation overhead.

use super::{EncodedPacket, Encoder, ResolvedEncoderConfig};
use crate::encode::{EncodeError, EncodeResult};
use bytes::Bytes;
use crossbeam::channel::{bounded, Receiver, Sender, TrySendError};
use image::{codecs::jpeg::JpegEncoder, ExtendedColorType};
use std::thread;
use tracing::{debug, info, warn};

// ── BGRA → JPEG conversion ──────────────────────────────────────────

/// Encode a BGRA frame slice into a JPEG byte vector.
///
/// `rgb_buf` is a caller-owned scratch buffer that is resized as needed
/// and reused across calls to eliminate per-frame allocation.
fn bgra_to_jpeg_reuse(
    bgra: &[u8],
    src_w: usize,
    src_h: usize,
    out_w: usize,
    out_h: usize,
    quality: u8,
    rgb_buf: &mut Vec<u8>,
) -> EncodeResult<Vec<u8>> {
    let expected_len = src_w * src_h * 4;
    if bgra.len() != expected_len {
        return Err(EncodeError::msg(format!(
            "Invalid BGRA size: got={}, expected={} ({}x{})",
            bgra.len(),
            expected_len,
            src_w,
            src_h
        )));
    }

    let rgb_len = out_w * out_h * 3;
    rgb_buf.resize(rgb_len, 0);

    if out_w == src_w && out_h == src_h {
        // Fast path – same resolution, just swap B/R channels
        // Optimized using slice iterators for better cache efficiency
        let mut dst_idx = 0;
        for src_idx in (0..bgra.len()).step_by(4) {
            rgb_buf[dst_idx] = bgra[src_idx + 2]; // R
            rgb_buf[dst_idx + 1] = bgra[src_idx + 1]; // G
            rgb_buf[dst_idx + 2] = bgra[src_idx]; // B
            dst_idx += 3;
        }
    } else {
        // Optimized bilinear scaling using floating-point arithmetic for better performance
        let x_ratio = src_w as f32 / out_w as f32;
        let y_ratio = src_h as f32 / out_h as f32;

        for dst_y in 0..out_h {
            let src_y = (dst_y as f32 + 0.5) * y_ratio - 0.5;
            let src_y_clamped = src_y.max(0.0).min((src_h - 1) as f32);
            let src_y0 = src_y_clamped.floor() as usize;
            let src_y1 = (src_y0 + 1).min(src_h - 1);
            let y_frac = src_y_clamped - src_y0 as f32;

            let dst_row_base = dst_y * out_w * 3;

            for dst_x in 0..out_w {
                let src_x = (dst_x as f32 + 0.5) * x_ratio - 0.5;
                let src_x_clamped = src_x.max(0.0).min((src_w - 1) as f32);
                let src_x0 = src_x_clamped.floor() as usize;
                let src_x1 = (src_x0 + 1).min(src_w - 1);
                let x_frac = src_x_clamped - src_x0 as f32;

                // Get the four neighboring pixels in BGRA
                let i00 = (src_y0 * src_w + src_x0) * 4;
                let i10 = (src_y0 * src_w + src_x1) * 4;
                let i01 = (src_y1 * src_w + src_x0) * 4;
                let i11 = (src_y1 * src_w + src_x1) * 4;

                let di = dst_row_base + dst_x * 3;

                // Bilinear interpolation
                for c in 0..3 {
                    let src_c = 2 - c; // Map RGB (0,1,2) to BGRA (2,1,0)

                    let v00 = bgra[i00 + src_c] as f32;
                    let v10 = bgra[i10 + src_c] as f32;
                    let v01 = bgra[i01 + src_c] as f32;
                    let v11 = bgra[i11 + src_c] as f32;

                    // Interpolate x first, then y
                    let v_top = v00 + (v10 - v00) * x_frac;
                    let v_bot = v01 + (v11 - v01) * x_frac;
                    let v = v_top + (v_bot - v_top) * y_frac;

                    rgb_buf[di + c] = v.clamp(0.0, 255.0) as u8;
                }
            }
        }
    }

    // Encode to JPEG – pre-allocate output based on typical compression ratio
    let mut out = Vec::with_capacity(rgb_len / 6);
    let mut enc = JpegEncoder::new_with_quality(&mut out, quality);
    enc.encode(rgb_buf, out_w as u32, out_h as u32, ExtendedColorType::Rgb8)
        .map_err(|e| EncodeError::msg(format!("JPEG encode: {}", e)))?;
    Ok(out)
}

/// Convenience wrapper that allocates its own scratch buffer (used by StubEncoder).
fn bgra_to_jpeg(
    frame: &crate::media::CapturedFrame,
    output_width: u32,
    output_height: u32,
    quality: u8,
) -> EncodeResult<Vec<u8>> {
    if frame.d3d11.is_some() {
        return Err(EncodeError::msg(
            "software JPEG encoding requires CPU-readable BGRA frames",
        ));
    }
    let (sw, sh) = frame.resolution;
    let mut rgb_buf = Vec::new();
    bgra_to_jpeg_reuse(
        &frame.bgra,
        sw as usize,
        sh as usize,
        output_width.max(1) as usize,
        output_height.max(1) as usize,
        quality,
        &mut rgb_buf,
    )
}

// ── Stub Encoder (no-FFmpeg build) ──────────────────────────────────

/// Minimal encoder that converts each frame to JPEG on the caller thread.
pub struct StubEncoder {
    config: ResolvedEncoderConfig,
    packet_rx: Receiver<EncodedPacket>,
    packet_tx: Sender<EncodedPacket>,
    frame_count: u64,
    running: bool,
}

impl StubEncoder {
    pub fn new(config: &ResolvedEncoderConfig) -> EncodeResult<Self> {
        info!("Creating stub encoder");
        let (tx, rx) = bounded(64);
        Ok(Self {
            config: config.clone(),
            packet_rx: rx,
            packet_tx: tx,
            frame_count: 0,
            running: false,
        })
    }
}

impl Encoder for StubEncoder {
    fn init(&mut self, config: &ResolvedEncoderConfig) -> EncodeResult<()> {
        self.config = config.clone();
        self.running = true;
        info!("Stub encoder initialized");
        Ok(())
    }

    fn encode_frame(&mut self, frame: &crate::media::CapturedFrame) -> EncodeResult<()> {
        let is_keyframe = self.frame_count % 30 == 0;

        // Use native resolution if configured, otherwise use config resolution
        let (out_w, out_h) = if self.config.use_native_resolution {
            frame.resolution
        } else {
            self.config.resolution
        };

        let data = bgra_to_jpeg(frame, out_w, out_h, 85)?;

        let mut packet = EncodedPacket::new(
            data,
            frame.timestamp,
            frame.timestamp,
            is_keyframe,
            super::StreamType::Video,
        );
        packet.resolution = Some(frame.resolution);

        let _ = self.packet_tx.try_send(packet);
        self.frame_count += 1;
        Ok(())
    }

    fn flush(&mut self) -> EncodeResult<Vec<EncodedPacket>> {
        self.running = false;
        Ok(vec![])
    }

    fn packet_rx(&self) -> Receiver<EncodedPacket> {
        self.packet_rx.clone()
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

// ── Software Encoder (parallel JPEG workers) ────────────────────────

/// Message sent to a worker thread. Uses `Bytes` for the BGRA payload
/// so passing it across threads is a cheap ref-count bump, not a 14 MB copy.
struct WorkItem {
    bgra: Bytes,
    src_w: u32,
    src_h: u32,
    timestamp: i64,
    resolution: (u32, u32),
    /// Whether to use native frame resolution (output = source)
    use_native_resolution: bool,
}

/// Multi-threaded software encoder.
///
/// `encode_frame` pushes work items to a bounded channel; a pool of
/// worker threads each maintain their own pre-allocated RGB buffer and
/// JPEG-compress frames in parallel.
pub struct SoftwareEncoder {
    config: ResolvedEncoderConfig,
    packet_rx: Receiver<EncodedPacket>,
    frame_count: u64,
    running: bool,
    worker_tx: Sender<WorkItem>,
    worker_rx: Receiver<WorkItem>,
    worker_threads: Vec<thread::JoinHandle<()>>,
    dropped_oldest_count: u64,
    dropped_newest_count: u64,
}

impl SoftwareEncoder {
    /// Create a new software encoder backed by a thread pool.
    pub fn new(config: &ResolvedEncoderConfig) -> EncodeResult<Self> {
        let num_workers = thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .clamp(2, 8);
        info!(
            "Creating software encoder with {} worker threads",
            num_workers
        );

        let (packet_tx, packet_rx) = bounded(32);
        let (worker_tx, worker_rx) = bounded::<WorkItem>(16);

        let out_w = config.resolution.0;
        let out_h = config.resolution.1;
        let quality = 85u8;

        let mut worker_threads = Vec::with_capacity(num_workers);
        for id in 0..num_workers {
            let rx = worker_rx.clone();
            let tx = packet_tx.clone();
            let ow = out_w as usize;
            let oh = out_h as usize;

            worker_threads.push(thread::spawn(move || {
                debug!("Encoder worker {id} started");

                // Each worker keeps its own RGB scratch buffer that is reused
                // across frames, eliminating repeated heap allocation.
                let mut rgb_buf: Vec<u8> = Vec::with_capacity(ow * oh * 3);

                while let Ok(item) = rx.recv() {
                    // Use native resolution if flag is set, otherwise use config resolution
                    let (target_w, target_h) = if item.use_native_resolution {
                        (item.src_w as usize, item.src_h as usize)
                    } else {
                        (ow, oh)
                    };

                    match bgra_to_jpeg_reuse(
                        &item.bgra,
                        item.src_w as usize,
                        item.src_h as usize,
                        target_w,
                        target_h,
                        quality,
                        &mut rgb_buf,
                    ) {
                        Ok(jpeg) => {
                            let mut pkt = EncodedPacket::new(
                                jpeg,
                                item.timestamp,
                                item.timestamp,
                                true, // Every MJPEG frame is a keyframe
                                super::StreamType::Video,
                            );
                            pkt.resolution = Some(item.resolution);
                            if tx.send(pkt).is_err() {
                                break;
                            }
                        }
                        Err(e) => warn!("Worker {id} encode error: {e}"),
                    }
                }

                debug!("Encoder worker {id} exiting");
            }));
        }

        Ok(Self {
            config: config.clone(),
            packet_rx,
            frame_count: 0,
            running: false,
            worker_tx,
            worker_rx,
            worker_threads,
            dropped_oldest_count: 0,
            dropped_newest_count: 0,
        })
    }
}

impl Encoder for SoftwareEncoder {
    fn init(&mut self, config: &ResolvedEncoderConfig) -> EncodeResult<()> {
        self.config = config.clone();
        self.running = true;
        info!(
            "Software encoder ready: {}x{} @ {} FPS, {} workers",
            config.resolution.0,
            config.resolution.1,
            config.framerate,
            self.worker_threads.len(),
        );
        Ok(())
    }

    fn encode_frame(&mut self, frame: &crate::media::CapturedFrame) -> EncodeResult<()> {
        if frame.d3d11.is_some() {
            return Err(EncodeError::msg(
                "software encoder cannot consume GPU-backed frames",
            ));
        }
        let item = WorkItem {
            bgra: frame.bgra.clone(), // Bytes clone = ref-count bump, O(1)
            src_w: frame.resolution.0,
            src_h: frame.resolution.1,
            timestamp: frame.timestamp,
            resolution: frame.resolution,
            use_native_resolution: self.config.use_native_resolution,
        };

        // Non-blocking send; on saturation, prefer dropping oldest queued work.
        match self.worker_tx.try_send(item) {
            Ok(()) => {}
            Err(TrySendError::Full(item)) => {
                let mut dropped_oldest = false;
                if self.worker_rx.try_recv().is_ok() {
                    dropped_oldest = true;
                    self.dropped_oldest_count = self.dropped_oldest_count.saturating_add(1);
                }

                if self.worker_tx.try_send(item).is_err() {
                    self.dropped_newest_count = self.dropped_newest_count.saturating_add(1);
                    if self.dropped_newest_count % 60 == 0 {
                        debug!(
                            "Workers saturated, dropped newest={} dropped_oldest={}",
                            self.dropped_newest_count, self.dropped_oldest_count
                        );
                    }
                } else if dropped_oldest && self.dropped_oldest_count % 60 == 0 {
                    debug!(
                        "Workers saturated, dropped oldest={} to keep recency",
                        self.dropped_oldest_count
                    );
                }
            }
            Err(TrySendError::Disconnected(_)) => {
                self.dropped_newest_count = self.dropped_newest_count.saturating_add(1);
                warn!("Software encoder worker queue disconnected");
            }
        }

        self.frame_count += 1;
        Ok(())
    }

    fn flush(&mut self) -> EncodeResult<Vec<EncodedPacket>> {
        info!(
            "Flushing software encoder ({} frames dispatched)",
            self.frame_count
        );
        // Workers drain naturally when worker_tx is dropped.
        Ok(vec![])
    }

    fn packet_rx(&self) -> Receiver<EncodedPacket> {
        self.packet_rx.clone()
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

impl Drop for SoftwareEncoder {
    fn drop(&mut self) {
        // Drop the sender half first: workers blocked on `rx.recv()` will receive a
        // disconnected error and exit their loops cleanly.
        let (dummy_tx, _) = crossbeam::channel::bounded(0);
        let _ = std::mem::replace(&mut self.worker_tx, dummy_tx);

        // Now join every worker so their stacks and `rgb_buf` scratch buffers are freed
        // before we return. This also surfaces any worker panics via the join result.
        for handle in self.worker_threads.drain(..) {
            if let Err(e) = handle.join() {
                warn!("Software encoder worker thread panicked: {:?}", e);
            }
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode::EncoderConfig;
    use crate::encode::ResolvedEncoderType;

    fn create_test_resolved() -> ResolvedEncoderConfig {
        let c = EncoderConfig::new(20, 30, (1920, 1080), crate::config::EncoderType::Amf, 1);
        ResolvedEncoderConfig {
            bitrate_mbps: c.bitrate_mbps,
            framerate: c.framerate,
            resolution: c.resolution,
            use_native_resolution: c.use_native_resolution,
            encoder_type: ResolvedEncoderType::Amf,
            quality_preset: c.quality_preset,
            rate_control: c.rate_control,
            quality_value: c.quality_value,
            keyframe_interval_secs: c.keyframe_interval_secs,
            use_cpu_readback: c.use_cpu_readback,
            output_index: c.output_index,
        }
    }

    #[test]
    fn test_stub_encoder_creation() {
        let config = create_test_resolved();
        assert!(StubEncoder::new(&config).is_ok());
    }

    #[test]
    fn test_software_encoder_creation() {
        let config = create_test_resolved();
        assert!(SoftwareEncoder::new(&config).is_ok());
    }

    #[test]
    fn test_encoder_codec_name() {
        let config = create_test_resolved();
        assert_eq!(config.ffmpeg_codec_name(), "hevc_amf");
    }
}
