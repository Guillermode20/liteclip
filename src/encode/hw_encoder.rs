//! Hardware Encoder Implementations (NVENC, AMF, QSV)
//!
//! Uses FFmpeg C API via ffmpeg-next crate for hardware-accelerated encoding.
//! Phase 1: CPU readback path - textures are copied to CPU before encoding.
//! Phase 3: Zero-copy GPU path - direct D3D11 texture to hardware encoder.

use super::{EncodedPacket, Encoder, EncoderConfig};
use anyhow::Result;
use crossbeam::channel::{bounded, Receiver, Sender};
use tracing::{info, trace};

/// NVENC encoder wrapper (stub for Phase 1)
pub struct NvencEncoder {
    #[allow(dead_code)]
    config: EncoderConfig,
    packet_rx: Receiver<EncodedPacket>,
    _packet_tx: Sender<EncodedPacket>,
    frame_count: u64,
    #[allow(dead_code)]
    running: bool,
}

impl NvencEncoder {
    /// Create new NVENC encoder
    pub fn new(config: &EncoderConfig) -> Result<Self> {
        info!("Creating NVENC encoder (stub)");
        let (tx, rx) = bounded(64);

        Ok(Self {
            config: config.clone(),
            packet_rx: rx,
            _packet_tx: tx,
            frame_count: 0,
            running: false,
        })
    }
}

impl Encoder for NvencEncoder {
    fn init(&mut self, _config: &EncoderConfig) -> Result<()> {
        info!("NVENC encoder initialized (stub)");
        self.running = true;
        Ok(())
    }

    fn encode_frame(&mut self, _frame: &crate::capture::CapturedFrame) -> Result<()> {
        trace!("NVENC encoding frame {}", self.frame_count);
        self.frame_count += 1;
        Ok(())
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>> {
        info!("Flushing NVENC encoder");
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

/// AMF encoder wrapper (AMD) (stub for Phase 1)
pub struct AmfEncoder {
    #[allow(dead_code)]
    config: EncoderConfig,
    packet_rx: Receiver<EncodedPacket>,
    _packet_tx: Sender<EncodedPacket>,
    frame_count: u64,
    #[allow(dead_code)]
    running: bool,
}

impl AmfEncoder {
    /// Create new AMF encoder
    pub fn new(config: &EncoderConfig) -> Result<Self> {
        info!("Creating AMF encoder (stub)");
        let (tx, rx) = bounded(64);

        Ok(Self {
            config: config.clone(),
            packet_rx: rx,
            _packet_tx: tx,
            frame_count: 0,
            running: false,
        })
    }
}

impl Encoder for AmfEncoder {
    fn init(&mut self, _config: &EncoderConfig) -> Result<()> {
        info!("AMF encoder initialized (stub)");
        self.running = true;
        Ok(())
    }

    fn encode_frame(&mut self, _frame: &crate::capture::CapturedFrame) -> Result<()> {
        trace!("AMF encoding frame {}", self.frame_count);
        self.frame_count += 1;
        Ok(())
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>> {
        info!("Flushing AMF encoder");
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

/// QSV encoder wrapper (Intel) (stub for Phase 1)
pub struct QsvEncoder {
    #[allow(dead_code)]
    config: EncoderConfig,
    packet_rx: Receiver<EncodedPacket>,
    _packet_tx: Sender<EncodedPacket>,
    frame_count: u64,
    #[allow(dead_code)]
    running: bool,
}

impl QsvEncoder {
    /// Create new QSV encoder
    pub fn new(config: &EncoderConfig) -> Result<Self> {
        info!("Creating QSV encoder (stub)");
        let (tx, rx) = bounded(64);

        Ok(Self {
            config: config.clone(),
            packet_rx: rx,
            _packet_tx: tx,
            frame_count: 0,
            running: false,
        })
    }
}

impl Encoder for QsvEncoder {
    fn init(&mut self, _config: &EncoderConfig) -> Result<()> {
        info!("QSV encoder initialized (stub)");
        self.running = true;
        Ok(())
    }

    fn encode_frame(&mut self, _frame: &crate::capture::CapturedFrame) -> Result<()> {
        trace!("QSV encoding frame {}", self.frame_count);
        self.frame_count += 1;
        Ok(())
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>> {
        info!("Flushing QSV encoder");
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
            crate::config::EncoderType::Nvenc,
            1,
        )
    }

    #[test]
    fn test_nvenc_encoder_creation() {
        let config = create_test_config();
        let encoder = NvencEncoder::new(&config);
        assert!(encoder.is_ok());
    }

    #[test]
    fn test_amf_encoder_creation() {
        let config = EncoderConfig::new(
            crate::config::Codec::H264,
            20,
            30,
            (1920, 1080),
            crate::config::EncoderType::Amf,
            1,
        );
        let encoder = AmfEncoder::new(&config);
        assert!(encoder.is_ok());
    }

    #[test]
    fn test_qsv_encoder_creation() {
        let config = EncoderConfig::new(
            crate::config::Codec::H264,
            20,
            30,
            (1920, 1080),
            crate::config::EncoderType::Qsv,
            1,
        );
        let encoder = QsvEncoder::new(&config);
        assert!(encoder.is_ok());
    }

    #[test]
    fn test_encoder_trait() {
        use super::super::Encoder;

        let config = create_test_config();
        let mut encoder = NvencEncoder::new(&config).unwrap();
        
        // Test init
        assert!(encoder.init(&config).is_ok());
        assert!(encoder.is_running());

        // Test flush
        let packets = encoder.flush().unwrap();
        assert!(packets.is_empty());
        assert!(!encoder.is_running());
    }
}
