use crate::{
    buffer::ReplayBuffer,
    capture::audio::{AudioLevelMonitor, WasapiAudioManager},
    config::Config,
};
use anyhow::{Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Handle for the audio forwarding thread.
///
/// This struct manages the lifecycle of the thread that forwards audio packets
/// from the audio manager to the replay buffer. It ensures proper shutdown
/// coordination and thread cleanup.
///
/// # Thread Lifecycle
///
/// - The forwarding thread checks `running` flag before each recv attempt
/// - When dropped, signals shutdown via `running` flag and joins the thread
/// - Thread exits gracefully when `running` is false OR channel is disconnected
pub struct AudioForwardHandle {
    /// Thread handle for the forwarding loop
    thread: Option<JoinHandle<()>>,
    /// Tracks whether the forwarding thread is expected to be alive
    running: Arc<AtomicBool>,
    /// Shutdown notifier to wake the forwarding thread immediately
    shutdown_tx: Option<crossbeam::channel::Sender<()>>,
}

impl AudioForwardHandle {
    /// Creates a new audio forward handle.
    fn new(
        thread: JoinHandle<()>,
        running: Arc<AtomicBool>,
        shutdown_tx: crossbeam::channel::Sender<()>,
    ) -> Self {
        Self {
            thread: Some(thread),
            running,
            shutdown_tx: Some(shutdown_tx),
        }
    }

    /// Signals the forwarding thread to stop and waits for it to finish.
    ///
    /// Uses a timeout to prevent indefinite hangs during shutdown.
    pub fn stop(&mut self) {
        if self.thread.is_none() {
            return;
        }

        // Signal shutdown
        self.running.store(false, Ordering::SeqCst);
        self.shutdown_tx.take();
        debug!("Signaling audio forwarding thread to stop");

        // Join with timeout to prevent indefinite hangs
        if let Some(thread) = self.thread.take() {
            // Use a timeout pattern: park the thread for up to 2 seconds
            let start = std::time::Instant::now();
            const JOIN_TIMEOUT: Duration = Duration::from_secs(2);

            while start.elapsed() < JOIN_TIMEOUT {
                if thread.is_finished() {
                    match thread.join() {
                        Ok(()) => debug!("Audio forwarding thread stopped cleanly"),
                        Err(e) => warn!("Audio forwarding thread panicked: {:?}", e),
                    }
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }

            // Thread didn't finish within timeout - warn and proceed
            warn!(
                "Audio forwarding thread did not stop within {} seconds, proceeding with shutdown",
                JOIN_TIMEOUT.as_secs()
            );
            // We still need to join to avoid leaking the thread handle
            match thread.join() {
                Ok(()) => {}
                Err(e) => warn!("Audio forwarding thread panicked after timeout: {:?}", e),
            }
        }
    }

    /// Check if the forwarding thread is still running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
            && self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }
}

impl Drop for AudioForwardHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Result of starting audio capture.
///
/// Contains both the audio manager (for control operations) and the
/// forward handle (for thread lifecycle management).
pub struct AudioCaptureResult {
    /// Audio capture manager
    pub manager: WasapiAudioManager,
    /// Handle for the forwarding thread (None if audio disabled)
    pub forward_handle: Option<AudioForwardHandle>,
}

/// Start audio capture and return both manager and forward handle.
///
/// This function spawns a forwarding thread that moves audio packets from
/// the audio manager to the replay buffer. The returned `AudioForwardHandle`
/// must be stored and used for cleanup when stopping the pipeline.
///
/// # Arguments
///
/// * `config` - Application configuration
/// * `buffer` - Replay buffer to forward packets to
/// * `context` - Context label for logging
/// * `level_monitor` - Optional audio level monitor for GUI
///
/// # Returns
///
/// An `AudioCaptureResult` containing:
/// - `manager`: The WASAPI audio manager
/// - `forward_handle`: Handle to the forwarding thread (for cleanup)
pub fn start_audio_capture(
    config: &Config,
    buffer: &ReplayBuffer,
    context: &str,
    level_monitor: Option<AudioLevelMonitor>,
) -> Result<AudioCaptureResult> {
    if !config.audio.capture_system && !config.audio.capture_mic {
        // Audio disabled - return manager without forwarding thread
        let manager = WasapiAudioManager::new()?;
        return Ok(AudioCaptureResult {
            manager,
            forward_handle: None,
        });
    }

    let mut audio_manager = WasapiAudioManager::with_level_monitor(level_monitor)
        .context("Failed to create audio manager")?;
    audio_manager
        .start(&config.audio)
        .context("Failed to start audio capture")?;

    let audio_packet_rx = audio_manager.packet_rx();
    let buffer_clone = buffer.clone();
    let context_label = context.to_string();
    let context_for_thread = context_label.clone();

    // Shutdown coordination
    let running = Arc::new(AtomicBool::new(true));
    let running_clone = Arc::clone(&running);
    let (shutdown_tx, shutdown_rx) = crossbeam::channel::bounded::<()>(1);

    let thread = std::thread::spawn(move || {
        let mut forwarded_packets = 0u64;
        let mut packet_batch = Vec::with_capacity(32);

        while running_clone.load(Ordering::SeqCst) {
            crossbeam::channel::select! {
                recv(shutdown_rx) -> _ => {
                    break;
                }
                recv(audio_packet_rx) -> recv_result => {
                    let packet = match recv_result {
                        Ok(packet) => packet,
                        Err(_) => {
                            debug!(
                                "Audio packet channel disconnected, exiting forwarding thread ({})",
                                context_for_thread
                            );
                            break;
                        }
                    };
                    packet_batch.push(packet);
                    forwarded_packets = forwarded_packets.saturating_add(1);

                    // Batch up to 32 packets
                    while packet_batch.len() < 32 {
                        match audio_packet_rx.try_recv() {
                            Ok(p) => {
                                packet_batch.push(p);
                                forwarded_packets = forwarded_packets.saturating_add(1);
                            }
                            Err(_) => break,
                        }
                    }

                    buffer_clone.push_batch(std::mem::take(&mut packet_batch).into_iter());

                    if forwarded_packets <= 32 {
                        debug!(
                            "Forwarded first audio packets to replay buffer ({})",
                            context_for_thread
                        );
                    } else if forwarded_packets % 500 < 32 {
                        debug!(
                            "Forwarded ~{} audio packets to replay buffer",
                            forwarded_packets
                        );
                    }
                }
            }
        }

        running_clone.store(false, Ordering::SeqCst);

        debug!(
            "Audio forwarding thread stopped after forwarding {} packets ({})",
            forwarded_packets, context_for_thread
        );
    });

    let forward_handle = AudioForwardHandle::new(thread, running, shutdown_tx);

    info!("Audio capture started ({})", context_label);
    Ok(AudioCaptureResult {
        manager: audio_manager,
        forward_handle: Some(forward_handle),
    })
}
