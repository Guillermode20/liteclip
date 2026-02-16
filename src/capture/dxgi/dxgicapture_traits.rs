//! # DxgiCapture - Trait Implementations
//!
//! This module contains trait implementations for `DxgiCapture`.
//!
//! ## Implemented Traits
//!
//! - `CaptureBackend`
//! - `Drop`
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use crate::capture::{CaptureBackend, CaptureConfig, CapturedFrame};
use anyhow::{bail, Result};
use crossbeam::channel::Receiver;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread::spawn;
use tracing::{error, info};

use super::types::DxgiCapture;

impl CaptureBackend for DxgiCapture {
    fn start(&mut self, config: CaptureConfig) -> Result<()> {
        if self.running.load(Ordering::Relaxed) {
            bail!("Capture already running");
        }
        info!("Starting DXGI capture: {} FPS", config.target_fps);
        self.config = config;
        self.running.store(true, Ordering::Relaxed);
        let running = Arc::clone(&self.running);
        let frame_tx = self._frame_tx.clone();
        let config = self.config.clone();
        self.capture_thread = Some(spawn(move || {
            Self::capture_loop(running, frame_tx, config);
        }));
        Ok(())
    }
    fn stop(&mut self) {
        if !self.running.load(Ordering::Relaxed) {
            return;
        }
        info!("Stopping DXGI capture...");
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.capture_thread.take() {
            if let Err(e) = handle.join() {
                error!("Capture thread join failed: {:?}", e);
            }
        }
        info!("DXGI capture stopped");
    }
    fn frame_rx(&self) -> Receiver<CapturedFrame> {
        self.frame_rx.clone()
    }
}

impl Drop for DxgiCapture {
    fn drop(&mut self) {
        self.stop();
    }
}
