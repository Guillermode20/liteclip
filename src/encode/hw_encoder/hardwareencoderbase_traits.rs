//! # HardwareEncoderBase - Trait Implementations
//!
//! This module contains trait implementations for `HardwareEncoderBase`.
//!
//! ## Implemented Traits
//!
//! - `Drop`
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use super::types::HardwareEncoderBase;
use tracing::warn;

/// Drop implementation to ensure FFmpeg process is cleaned up
impl Drop for HardwareEncoderBase {
    fn drop(&mut self) {
        drop(self.ffmpeg_stdin.take());
        if let Some(mut child) = self.ffmpeg_process.take() {
            match child.try_wait() {
                Ok(Some(_)) => {}
                _ => {
                    warn!("FFmpeg process still running during drop, killing");
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }
        if let Some(handle) = self.stdout_thread.take() {
            if handle.is_finished() {
                let _ = handle.join();
            }
        }
        if let Some(handle) = self.stderr_thread.take() {
            if handle.is_finished() {
                let _ = handle.join();
            }
        }
    }
}

