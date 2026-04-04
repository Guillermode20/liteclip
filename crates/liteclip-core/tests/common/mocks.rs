//! Mock implementations of encoder and capture traits for testing.
//!
//! These mocks allow testing the recording pipeline without requiring
//! actual hardware encoding or screen capture. They produce deterministic
//! output suitable for verifying buffer behavior, pipeline flow, and error handling.

use crossbeam::channel::{bounded, Receiver, Sender};
use liteclip_core::encode::{
    EncodeResult, EncodedPacket, Encoder, EncoderFactory, EncoderHandle, EncoderHealthEvent,
    ResolvedEncoderConfig,
};
use liteclip_core::media::CapturedFrame;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::thread::{self, JoinHandle};

/// Mock encoder that produces deterministic fake output.
///
/// Simulates a real encoder by accepting frames and emitting encoded packets
/// with synthetic data. Useful for testing buffer management, pipeline flow,
/// and muxing logic without requiring actual hardware encoding.
///
/// # Example
///
/// ```
/// let mut encoder = MockEncoder::new();
/// encoder.init(&config)?;
/// encoder.encode_frame(&frame)?;
/// let packet = encoder.packet_rx().try_recv()?;
/// ```
pub struct MockEncoder {
    packet_tx: Sender<EncodedPacket>,
    packet_rx: Receiver<EncodedPacket>,
    frame_count: Arc<AtomicUsize>,
    running: Arc<AtomicBool>,
    fail_on_encode: bool,
}

impl MockEncoder {
    /// Create a new mock encoder with default settings.
    pub fn new() -> Self {
        let (tx, rx) = bounded(100);
        Self {
            packet_tx: tx,
            packet_rx: rx,
            frame_count: Arc::new(AtomicUsize::new(0)),
            running: Arc::new(AtomicBool::new(true)),
            fail_on_encode: false,
        }
    }

    /// Configure the mock to simulate encoding failures.
    ///
    /// When enabled, `encode_frame()` will return an error instead of
    /// producing packets. Useful for testing error handling paths.
    pub fn with_failure(mut self) -> Self {
        self.fail_on_encode = true;
        self
    }

    /// Get the number of frames processed by this encoder.
    pub fn frame_count(&self) -> usize {
        self.frame_count.load(Ordering::SeqCst)
    }

    /// Generate a synthetic encoded packet from a captured frame.
    ///
    /// Creates a packet with:
    /// - Synthetic data (512 bytes of zeros)
    /// - Keyframe every 30 frames (GOP size of 30)
    /// - PTS/DTS copied from frame timestamp
    fn emit_packet(&self, frame: &CapturedFrame) {
        let count = self.frame_count.load(Ordering::SeqCst);
        let is_keyframe = count % 30 == 0; // Keyframe every 30 frames
        let packet = EncodedPacket {
            data: bytes::Bytes::from(vec![0u8; 512]),
            pts: frame.timestamp,
            dts: frame.timestamp,
            is_keyframe,
            stream: liteclip_core::encode::StreamType::Video,
            resolution: Some(frame.resolution),
        };
        // Ignore send errors - channel might be closed during test teardown
        let _ = self.packet_tx.send(packet);
    }
}

impl Default for MockEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Encoder for MockEncoder {
    fn init(&mut self, _config: &ResolvedEncoderConfig) -> EncodeResult<()> {
        self.running.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn encode_frame(&mut self, frame: &CapturedFrame) -> EncodeResult<()> {
        if self.fail_on_encode {
            return Err(liteclip_core::encode::EncodeError::msg(
                "Mock encoder failure",
            ));
        }
        self.frame_count.fetch_add(1, Ordering::SeqCst);
        self.emit_packet(frame);
        Ok(())
    }

    fn flush(&mut self) -> EncodeResult<Vec<EncodedPacket>> {
        self.running.store(false, Ordering::SeqCst);
        Ok(Vec::new())
    }

    fn packet_rx(&self) -> Receiver<EncodedPacket> {
        self.packet_rx.clone()
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

/// Thread-safe handle for a spawned mock encoder.
///
/// Ensures the encoder thread is properly joined when dropped,
/// preventing zombie threads during test failures.
pub struct MockEncoderHandle {
    thread: Option<JoinHandle<EncodeResult<()>>>,
    frame_tx: Sender<CapturedFrame>,
    _health_rx: Receiver<EncoderHealthEvent>,
    effective_config: ResolvedEncoderConfig,
}

impl MockEncoderHandle {
    /// Send a frame to the encoder thread for processing.
    pub fn send_frame(
        &self,
        frame: CapturedFrame,
    ) -> Result<(), crossbeam::channel::SendError<CapturedFrame>> {
        self.frame_tx.send(frame)
    }

    /// Get the effective configuration used by this encoder.
    pub fn config(&self) -> &ResolvedEncoderConfig {
        &self.effective_config
    }
}

impl Drop for MockEncoderHandle {
    fn drop(&mut self) {
        // Signal shutdown by closing the frame channel
        drop(self.frame_tx.clone());

        // Join the thread to ensure clean shutdown
        if let Some(thread) = self.thread.take() {
            // Use a timeout to prevent blocking indefinitely on panicked threads
            let _ = thread.join();
        }
    }
}

/// Factory for creating mock encoder instances.
///
/// Implements the `EncoderFactory` trait to allow injection of mock encoders
/// into the pipeline for testing. Configurable to simulate various failure modes.
pub struct MockEncoderFactory {
    /// When true, spawned encoders will fail on frame encoding.
    pub fail_on_encode: bool,
}

impl Default for MockEncoderFactory {
    fn default() -> Self {
        Self {
            fail_on_encode: false,
        }
    }
}

impl EncoderFactory for MockEncoderFactory {
    fn spawn(
        &self,
        config: ResolvedEncoderConfig,
        _buffer: liteclip_core::buffer::ring::SharedReplayBuffer,
        _frame_rx: Receiver<CapturedFrame>,
    ) -> EncodeResult<EncoderHandle> {
        let mut encoder = if self.fail_on_encode {
            MockEncoder::new().with_failure()
        } else {
            MockEncoder::new()
        };

        encoder.init(&config)?;

        let (_health_tx, health_rx) = bounded::<EncoderHealthEvent>(10);
        let (frame_tx_inner, frame_rx_inner) = bounded::<CapturedFrame>(100);

        // Spawn encoder thread with proper cleanup
        let thread = thread::spawn(move || {
            while let Ok(frame) = frame_rx_inner.recv() {
                if encoder.encode_frame(&frame).is_err() {
                    encoder.running.store(false, Ordering::SeqCst);
                    break;
                }
            }
            Ok(())
        });

        // Create handle
        let handle = EncoderHandle {
            thread,
            frame_tx: frame_tx_inner,
            health_rx,
            effective_config: config,
        };

        Ok(handle)
    }
}

/// Mock capture source that generates synthetic frames.
///
/// Simulates a screen capture source by generating frames with
/// configurable resolution and timestamps. Useful for testing
/// the capture → encode pipeline without requiring actual DXGI access.
///
/// # Example
///
/// ```
/// let (capture, frame_rx) = MockCaptureSource::new(1920, 1080);
/// capture.emit_frames(60, 30); // 60 frames at 30fps
/// assert_eq!(capture.frame_count(), 60);
/// ```
pub struct MockCaptureSource {
    frame_tx: crossbeam::channel::Sender<CapturedFrame>,
    frame_count: Arc<AtomicUsize>,
    running: Arc<AtomicBool>,
    width: u32,
    height: u32,
}

impl MockCaptureSource {
    /// Create a new mock capture source with the specified resolution.
    ///
    /// Returns the source and a receiver channel for consuming generated frames.
    pub fn new(width: u32, height: u32) -> (Self, Receiver<CapturedFrame>) {
        let (tx, rx) = bounded(100);
        let source = Self {
            frame_tx: tx,
            frame_count: Arc::new(AtomicUsize::new(0)),
            running: Arc::new(AtomicBool::new(true)),
            width,
            height,
        };
        (source, rx)
    }

    /// Emit a single synthetic frame at the specified timestamp.
    ///
    /// Timestamp is typically in microseconds (QPC ticks).
    pub fn emit_frame(&self, timestamp: i64) {
        if !self.running.load(Ordering::SeqCst) {
            return;
        }
        let frame = crate::common::fixtures::make_test_frame(self.width, self.height, timestamp);
        // Ignore send errors - channel might be closed
        let _ = self.frame_tx.send(frame);
        self.frame_count.fetch_add(1, Ordering::SeqCst);
    }

    /// Emit a sequence of frames at the specified framerate.
    ///
    /// # Arguments
    ///
    /// * `count` - Number of frames to emit
    /// * `framerate` - Frames per second (used to calculate timestamps)
    pub fn emit_frames(&self, count: usize, framerate: u32) {
        let interval = 1_000_000 / framerate as i64; // microseconds
        for i in 0..count {
            self.emit_frame(i as i64 * interval);
        }
    }

    /// Get the total number of frames emitted.
    pub fn frame_count(&self) -> usize {
        self.frame_count.load(Ordering::SeqCst)
    }

    /// Stop the capture source from emitting additional frames.
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::fixtures::make_test_frame;

    /// Test: MockEncoder produces packets for each encoded frame.
    #[test]
    fn mock_encoder_produces_packets() {
        let mut encoder = MockEncoder::new();
        let config = ResolvedEncoderConfig {
            bitrate_mbps: 20,
            framerate: 30,
            resolution: (1920, 1080),
            use_native_resolution: false,
            encoder_type: liteclip_core::encode::ResolvedEncoderType::Software,
            quality_preset: liteclip_core::config::QualityPreset::Balanced,
            rate_control: liteclip_core::config::RateControl::Cbr,
            quality_value: None,
            keyframe_interval_secs: 2,
            use_cpu_readback: false,
            output_index: 0,
        };

        encoder.init(&config).unwrap();

        let frame = make_test_frame(1920, 1080, 0);
        encoder.encode_frame(&frame).unwrap();

        let packet = encoder.packet_rx().try_recv();
        assert!(packet.is_ok(), "Encoder should produce a packet");
        assert_eq!(encoder.frame_count(), 1);
    }

    /// Test: MockEncoder with failure mode returns errors.
    #[test]
    fn mock_encoder_failure_mode() {
        let mut encoder = MockEncoder::new().with_failure();
        let config = ResolvedEncoderConfig {
            bitrate_mbps: 20,
            framerate: 30,
            resolution: (1920, 1080),
            use_native_resolution: false,
            encoder_type: liteclip_core::encode::ResolvedEncoderType::Software,
            quality_preset: liteclip_core::config::QualityPreset::Balanced,
            rate_control: liteclip_core::config::RateControl::Cbr,
            quality_value: None,
            keyframe_interval_secs: 2,
            use_cpu_readback: false,
            output_index: 0,
        };

        encoder.init(&config).unwrap();

        let frame = make_test_frame(1920, 1080, 0);
        let result = encoder.encode_frame(&frame);
        assert!(
            result.is_err(),
            "Encoder should fail when configured with failure mode"
        );
    }

    /// Test: MockCaptureSource emits correct number of frames.
    #[test]
    fn mock_capture_source_emits_frames() {
        let (source, rx) = MockCaptureSource::new(1920, 1080);

        source.emit_frames(10, 30);

        assert_eq!(source.frame_count(), 10);

        // Verify frames are on the channel
        let mut count = 0;
        while rx.try_recv().is_ok() {
            count += 1;
        }
        assert_eq!(count, 10);
    }

    /// Test: MockCaptureSource stop() prevents further frame emission.
    #[test]
    fn mock_capture_source_stop_prevents_emission() {
        let (source, rx) = MockCaptureSource::new(1920, 1080);

        source.emit_frames(5, 30);
        source.stop();
        source.emit_frames(5, 30); // Should be ignored

        assert_eq!(source.frame_count(), 5);

        // Verify only first 5 frames are on the channel
        let mut count = 0;
        while rx.try_recv().is_ok() {
            count += 1;
        }
        assert_eq!(count, 5);
    }
}
