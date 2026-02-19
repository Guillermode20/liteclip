//! # ManagedFfmpegProcess - Trait Implementations
//!
//! This module contains trait implementations for `ManagedFfmpegProcess`.
//!
//! ## Implemented Traits
//!
//! - `Drop`
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use std::time::{Duration, Instant};
use tracing::warn;

use super::types::ManagedFfmpegProcess;

impl Drop for ManagedFfmpegProcess {
    fn drop(&mut self) {
        if let Some(stdin) = self.stdin.take() {
            drop(stdin);
        }
        let process_timeout = Duration::from_secs(10);
        let start = Instant::now();
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        warn!("FFmpeg process exited with status: {}", status);
                    }
                    break;
                }
                Ok(None) => {
                    if start.elapsed() > process_timeout {
                        warn!(
                            "FFmpeg process did not exit within {:?}, killing",
                            process_timeout
                        );
                        let _ = self.child.kill();
                        let _ = self.child.wait();
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    warn!("Error waiting for FFmpeg process: {}", e);
                    break;
                }
            }
        }
        let thread_timeout = Duration::from_secs(5);
        if let Some(stdout_reader) = self.stdout_reader.take() {
            let start = Instant::now();
            while !stdout_reader.is_finished() {
                if start.elapsed() > thread_timeout {
                    warn!(
                        "stdout reader thread did not finish within {:?}", thread_timeout
                    );
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            if stdout_reader.is_finished() {
                let _ = stdout_reader.join();
            }
        }
        if let Some(stderr_reader) = self.stderr_reader.take() {
            let start = Instant::now();
            while !stderr_reader.is_finished() {
                if start.elapsed() > thread_timeout {
                    warn!(
                        "stderr reader thread did not finish within {:?}", thread_timeout
                    );
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            if stderr_reader.is_finished() {
                let _ = stderr_reader.join();
            }
        }
    }
}

