//! Async frame writer for decoupling capture from FFmpeg stdin writes.
//!
//! When FFmpeg is under GPU strain, `stdin.write_all()` can block. This module
//! provides a non-blocking queue + dedicated writer thread to prevent the
//! encoder thread from stalling.

use bytes::Bytes;
use crossbeam::channel::{bounded, RecvTimeoutError, Sender, TrySendError};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tracing::{debug, warn};

pub struct PendingFrame {
    pub data: Bytes,
    pub timestamp: i64,
}

pub struct AsyncFrameWriter {
    frame_tx: Sender<PendingFrame>,
    write_thread: Option<thread::JoinHandle<()>>,
    running: Arc<AtomicBool>,
    last_write_latency_ms: Arc<AtomicU32>,
}

impl AsyncFrameWriter {
    pub fn new<W: Write + Send + 'static>(stdin: W, queue_size: usize) -> Self {
        let (frame_tx, frame_rx) = bounded::<PendingFrame>(queue_size);
        let running = Arc::new(AtomicBool::new(true));
        let last_write_latency_ms = Arc::new(AtomicU32::new(0));

        let running_clone = running.clone();
        let latency_clone = last_write_latency_ms.clone();

        let write_thread = thread::Builder::new()
            .name("ffmpeg-writer".to_string())
            .spawn(move || {
                let mut frames_written = 0u64;
                let frames_dropped = 0u64;
                let mut stdin = stdin;
                
                while running_clone.load(Ordering::Relaxed) {
                    match frame_rx.recv_timeout(Duration::from_millis(50)) {
                        Ok(frame) => {
                            let start = std::time::Instant::now();
                            match stdin.write_all(&frame.data) {
                                Ok(()) => {
                                    frames_written += 1;
                                    let latency_ms = start.elapsed().as_millis() as u32;
                                    latency_clone.store(latency_ms, Ordering::Relaxed);
                                    
                                    if frames_written % 300 == 0 {
                                        debug!(
                                            "Async writer: {} frames written, {} dropped, latency={}ms",
                                            frames_written, frames_dropped, latency_ms
                                        );
                                    }
                                }
                                Err(e) => {
                                    warn!("FFmpeg stdin write error: {}", e);
                                    break;
                                }
                            }
                        }
                        Err(RecvTimeoutError::Timeout) => {
                            continue;
                        }
                        Err(RecvTimeoutError::Disconnected) => {
                            debug!("Async writer channel disconnected");
                            break;
                        }
                    }
                }
                
                debug!(
                    "Async writer stopped: {} frames written, {} dropped",
                    frames_written, frames_dropped
                );
            })
            .expect("Failed to spawn writer thread");

        Self {
            frame_tx,
            write_thread: Some(write_thread),
            running,
            last_write_latency_ms,
        }
    }

    pub fn try_queue(&self, frame: PendingFrame) -> Result<(), TrySendError<PendingFrame>> {
        self.frame_tx.try_send(frame)
    }

    pub fn write_latency_ms(&self) -> u32 {
        self.last_write_latency_ms.load(Ordering::Relaxed)
    }

    pub fn is_slow(&self) -> bool {
        self.write_latency_ms() > 16
    }

    pub fn queue_capacity(&self) -> usize {
        self.frame_tx.capacity().unwrap_or(0)
    }
}

impl Drop for AsyncFrameWriter {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.write_thread.take() {
            let _ = handle.join();
        }
    }
}
