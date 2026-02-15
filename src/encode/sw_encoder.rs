//! Software Encoder
//!
//! Parallel JPEG compression pipeline. Uses a thread pool of workers to
//! encode BGRA frames into MJPEG packets for the ring buffer.
//! Each worker pre-allocates its RGB conversion buffer to avoid per-frame
//! allocation overhead.

use super::{EncodedPacket, Encoder, EncoderConfig};
use anyhow::Result;
use bytes::Bytes;
use crossbeam::channel::{bounded, Receiver, Sender};
use image::{codecs::jpeg::JpegEncoder, ExtendedColorType};
use std::thread;
use tracing::{info, trace, warn};

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
) -> Result<Vec<u8>> {
    let expected_len = src_w * src_h * 4;
    if bgra.len() != expected_len {
        anyhow::bail!(
            "Invalid BGRA size: got={}, expected={} ({}x{})",
            bgra.len(),
            expected_len,
            src_w,
            src_h
        );
    }

    let rgb_len = out_w * out_h * 3;
    rgb_buf.resize(rgb_len, 0);

    if out_w == src_w && out_h == src_h {
        // Fast path – same resolution, just swap B/R channels
        for (src, dst) in bgra.chunks_exact(4).zip(rgb_buf.chunks_exact_mut(3)) {
            dst[0] = src[2]; // R
            dst[1] = src[1]; // G
            dst[2] = src[0]; // B
        }
    } else {
        // Nearest-neighbour downscale with precomputed X lookup table
        let mut x_map: Vec<usize> = Vec::with_capacity(out_w);
        for x in 0..out_w {
            x_map.push((x * src_w / out_w) * 4);
        }

        for y in 0..out_h {
            let src_row_base = (y * src_h / out_h) * src_w * 4;
            let dst_row_base = y * out_w * 3;

            let src_row = &bgra[src_row_base..src_row_base + src_w * 4];
            let dst_row = &mut rgb_buf[dst_row_base..dst_row_base + out_w * 3];

            for x in 0..out_w {
                let si = x_map[x];
                let di = x * 3;
                dst_row[di] = src_row[si + 2]; // R
                dst_row[di + 1] = src_row[si + 1]; // G
                dst_row[di + 2] = src_row[si]; // B
            }
        }
    }

    // Encode to JPEG – pre-allocate output based on typical compression ratio
    let mut out = Vec::with_capacity(rgb_len / 6);
    let mut enc = JpegEncoder::new_with_quality(&mut out, quality);
    enc.encode(rgb_buf, out_w as u32, out_h as u32, ExtendedColorType::Rgb8)?;
    Ok(out)
}

/// Convenience wrapper that allocates its own scratch buffer (used by StubEncoder).
fn bgra_to_jpeg(
    frame: &crate::capture::CapturedFrame,
    output_width: u32,
    output_height: u32,
    quality: u8,
) -> Result<Vec<u8>> {
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
    config: EncoderConfig,
    packet_rx: Receiver<EncodedPacket>,
    packet_tx: Sender<EncodedPacket>,
    frame_count: u64,
    running: bool,
}

impl StubEncoder {
    pub fn new(config: &EncoderConfig) -> Result<Self> {
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
    fn init(&mut self, config: &EncoderConfig) -> Result<()> {
        self.config = config.clone();
        self.running = true;
        info!("Stub encoder initialized");
        Ok(())
    }

    fn encode_frame(&mut self, frame: &crate::capture::CapturedFrame) -> Result<()> {
        trace!("Stub encoder frame {}", self.frame_count);

        let is_keyframe = self.frame_count % 30 == 0;
        let data = bgra_to_jpeg(
            frame,
            self.config.resolution.0,
            self.config.resolution.1,
            85,
        )?;

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

    fn flush(&mut self) -> Result<Vec<EncodedPacket>> {
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
}

/// Multi-threaded software encoder.
///
/// `encode_frame` pushes work items to a bounded channel; a pool of
/// worker threads each maintain their own pre-allocated RGB buffer and
/// JPEG-compress frames in parallel.
pub struct SoftwareEncoder {
    config: EncoderConfig,
    packet_rx: Receiver<EncodedPacket>,
    #[allow(dead_code)]
    packet_tx: Sender<EncodedPacket>,
    frame_count: u64,
    running: bool,
    worker_tx: Sender<WorkItem>,
    #[allow(dead_code)]
    worker_threads: Vec<thread::JoinHandle<()>>,
}

impl SoftwareEncoder {
    /// Create a new software encoder backed by a thread pool.
    pub fn new(config: &EncoderConfig) -> Result<Self> {
        let num_workers = thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .clamp(2, 8);
        info!(
            "Creating software encoder with {} worker threads",
            num_workers
        );

        let (packet_tx, packet_rx) = bounded(128);
        // Bounded to num_workers so we never enqueue more work than can be
        // processed in parallel – excess frames are dropped at the caller.
        let (worker_tx, worker_rx) = bounded::<WorkItem>(num_workers);

        let out_w = config.resolution.0;
        let out_h = config.resolution.1;
        let quality = 85u8; // JPEG quality 0-100; 85 = good fidelity, fast encode

        let mut worker_threads = Vec::with_capacity(num_workers);
        for id in 0..num_workers {
            let rx = worker_rx.clone();
            let tx = packet_tx.clone();
            let ow = out_w as usize;
            let oh = out_h as usize;

            worker_threads.push(thread::spawn(move || {
                trace!("Encoder worker {id} started");

                // Each worker keeps its own RGB scratch buffer that is reused
                // across frames, eliminating repeated heap allocation.
                let mut rgb_buf: Vec<u8> = Vec::with_capacity(ow * oh * 3);

                while let Ok(item) = rx.recv() {
                    match bgra_to_jpeg_reuse(
                        &item.bgra,
                        item.src_w as usize,
                        item.src_h as usize,
                        ow,
                        oh,
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

                trace!("Encoder worker {id} exiting");
            }));
        }

        Ok(Self {
            config: config.clone(),
            packet_rx,
            packet_tx,
            frame_count: 0,
            running: false,
            worker_tx,
            worker_threads,
        })
    }
}

impl Encoder for SoftwareEncoder {
    fn init(&mut self, config: &EncoderConfig) -> Result<()> {
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

    fn encode_frame(&mut self, frame: &crate::capture::CapturedFrame) -> Result<()> {
        let item = WorkItem {
            bgra: frame.bgra.clone(), // Bytes clone = ref-count bump, O(1)
            src_w: frame.resolution.0,
            src_h: frame.resolution.1,
            timestamp: frame.timestamp,
            resolution: frame.resolution,
        };

        // Non-blocking send; if all workers are busy we drop the frame
        // rather than stalling the capture pipeline.
        if self.worker_tx.try_send(item).is_err() {
            trace!("Workers busy, dropping frame {}", self.frame_count);
        }

        self.frame_count += 1;
        Ok(())
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>> {
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

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> EncoderConfig {
        EncoderConfig::new(
            crate::config::Codec::H264,
            20,
            30,
            (1920, 1080),
            crate::config::EncoderType::Software,
            1,
        )
    }

    #[test]
    fn test_stub_encoder_creation() {
        let config = create_test_config();
        assert!(StubEncoder::new(&config).is_ok());
    }

    #[test]
    fn test_software_encoder_creation() {
        let config = create_test_config();
        assert!(SoftwareEncoder::new(&config).is_ok());
    }

    #[test]
    fn test_encoder_codec_name() {
        let config = create_test_config();
        assert_eq!(config.ffmpeg_codec_name(), "libx264");
    }
}
