//! # QsvEncoder - Trait Implementations
//!
//! This module contains trait implementations for `QsvEncoder`.
//!
//! ## Implemented Traits
//!
//! - `Encoder`
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use crate::encode::{EncodedPacket, Encoder, EncoderConfig};
use crate::capture::CapturedFrame;
use anyhow::Result;
use crossbeam::channel::Receiver;
use tracing::debug;

use super::types::QsvEncoder;

impl Encoder for QsvEncoder {
    fn init(&mut self, config: &EncoderConfig) -> Result<()> {
        self.base.config = config.clone();
        self.base.running = true;
        if !self.base.config.use_cpu_readback && self.base.ffmpeg_process.is_none() {
            self.base.init_ffmpeg(0, 0)?;
        }
        debug!("QSV encoder initialized");
        Ok(())
    }
    fn encode_frame(
        &mut self,
        frame: &CapturedFrame,
    ) -> Result<()> {
        self.base.encode_frame_internal(frame)
    }
    fn flush(&mut self) -> Result<Vec<EncodedPacket>> {
        self.base.flush_internal()
    }
    fn packet_rx(&self) -> Receiver<EncodedPacket> {
        self.base.packet_rx.clone()
    }
    fn is_running(&self) -> bool {
        self.base.running
    }
}

