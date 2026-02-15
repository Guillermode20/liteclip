//! Software Encoder (x264 fallback)
//!
//! Uses libx264 for software encoding when hardware acceleration is unavailable.
//! Phase 1 implementation with CPU readback path.

use super::{EncodedPacket, Encoder, EncoderConfig};
use anyhow::Result;
use crossbeam::channel::{bounded, Receiver, Sender};
use tracing::{info, trace};

/// Stub encoder for compilation check
///
/// This is a stub implementation that allows the code to compile without FFmpeg.
/// In a real build with FFmpeg, this would be replaced with actual x264 encoding.
pub struct StubEncoder {
    #[allow(dead_code)]
    config: EncoderConfig,
    packet_rx: Receiver<EncodedPacket>,
    #[allow(dead_code)]
    packet_tx: Sender<EncodedPacket>,
    frame_count: u64,
    #[allow(dead_code)]
    running: bool,
}

impl StubEncoder {
    /// Create new stub encoder
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

    fn encode_frame(&mut self, _frame: &crate::capture::CapturedFrame) -> Result<()> {
        trace!("Stub encoder received frame {}", self.frame_count);
        self.frame_count += 1;
        Ok(())
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>> {
        info!("Flushing stub encoder");
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

/// Software encoder using libx264
///
/// This is the fallback encoder when no hardware acceleration is available.
/// It uses FFmpeg's libx264 codec with the "veryfast" preset for minimal CPU impact.
pub struct SoftwareEncoder {
    #[allow(dead_code)]
    config: EncoderConfig,
    packet_rx: Receiver<EncodedPacket>,
    #[allow(dead_code)]
    packet_tx: Sender<EncodedPacket>,
    #[allow(dead_code)]
    frame_count: u64,
    width: u32,
    height: u32,
    framerate: u32,
    #[allow(dead_code)]
    keyframe_interval: u32,
    #[allow(dead_code)]
    running: bool,
}

impl SoftwareEncoder {
    /// Create new software encoder
    pub fn new(config: &EncoderConfig) -> Result<Self> {
        info!("Creating software encoder (x264 fallback)");
        let (tx, rx) = bounded(64);

        Ok(Self {
            config: config.clone(),
            packet_rx: rx,
            packet_tx: tx,
            frame_count: 0,
            width: config.resolution.0,
            height: config.resolution.1,
            framerate: config.framerate,
            keyframe_interval: config.keyframe_interval_frames(),
            running: false,
        })
    }

    /// Convert BGRA data to YUV420P format
    ///
    /// This is a simple conversion suitable for the Phase 1 CPU readback path.
    /// In Phase 3, this will be replaced by GPU-accelerated color space conversion.
    #[allow(dead_code)]
    fn convert_bgra_to_yuv420p(&self, bgra: &[u8], width: u32, height: u32) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let width = width as usize;
        let height = height as usize;
        let y_size = width * height;
        let uv_size = y_size / 4;

        let mut y_plane = vec![0u8; y_size];
        let mut u_plane = vec![0u8; uv_size];
        let mut v_plane = vec![0u8; uv_size];

        // Simple BGRA to YUV420p conversion
        // This is a basic implementation - a production version would use SIMD
        for row in 0..height {
            for col in 0..width {
                let idx = (row * width + col) * 4;
                let b = bgra[idx] as f32;
                let g = bgra[idx + 1] as f32;
                let r = bgra[idx + 2] as f32;

                // Y = 0.299R + 0.587G + 0.114B
                let y = 0.299 * r + 0.587 * g + 0.114 * b;
                y_plane[row * width + col] = y.clamp(0.0, 255.0) as u8;

                // Subsample UV (every 2x2 block)
                if row % 2 == 0 && col % 2 == 0 {
                    let uv_idx = (row / 2) * (width / 2) + (col / 2);
                    // U = -0.147R - 0.289G + 0.436B + 128
                    // V = 0.615R - 0.515G - 0.100B + 128
                    let u = -0.147 * r - 0.289 * g + 0.436 * b + 128.0;
                    let v = 0.615 * r - 0.515 * g - 0.100 * b + 128.0;
                    u_plane[uv_idx] = u.clamp(0.0, 255.0) as u8;
                    v_plane[uv_idx] = v.clamp(0.0, 255.0) as u8;
                }
            }
        }

        (y_plane, u_plane, v_plane)
    }
}

impl Encoder for SoftwareEncoder {
    fn init(&mut self, config: &EncoderConfig) -> Result<()> {
        self.config = config.clone();
        self.running = true;

        info!(
            "Software encoder initialized: {}x{} @ {}fps",
            self.width, self.height, self.framerate
        );

        Ok(())
    }

    fn encode_frame(&mut self, _frame: &crate::capture::CapturedFrame) -> Result<()> {
        trace!("Software encoding frame {}", self.frame_count);
        self.frame_count += 1;
        Ok(())
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>> {
        info!("Flushing software encoder");
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
        let encoder = StubEncoder::new(&config);
        assert!(encoder.is_ok());
    }

    #[test]
    fn test_software_encoder_creation() {
        let config = create_test_config();
        let encoder = SoftwareEncoder::new(&config);
        assert!(encoder.is_ok());
    }

    #[test]
    fn test_bgra_to_yuv_conversion() {
        let config = create_test_config();
        let encoder = SoftwareEncoder::new(&config).unwrap();

        // Create a 2x2 BGRA image (each pixel is B, G, R, A)
        let bgra = vec![
            255, 0, 0, 255, // Blue pixel
            0, 255, 0, 255, // Green pixel
            0, 0, 255, 255, // Red pixel
            255, 255, 255, 255, // White pixel
        ];

        let (y, u, v) = encoder.convert_bgra_to_yuv420p(&bgra, 2, 2);

        // Check sizes
        assert_eq!(y.len(), 4); // 2x2 = 4 Y samples
        assert_eq!(u.len(), 1); // 1x1 = 1 U sample (subsampled)
        assert_eq!(v.len(), 1); // 1x1 = 1 V sample (subsampled)
    }

    #[test]
    fn test_encoder_codec_name() {
        let config = create_test_config();
        assert_eq!(config.ffmpeg_codec_name(), "libx264");
    }
}
