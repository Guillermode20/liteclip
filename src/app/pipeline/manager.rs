use crate::{
    buffer::ReplayBuffer, capture::dxgi::DxgiCapture, config::Config, encode::EncoderHandle,
};
use anyhow::Result;
use tracing::{error, info, warn};

use super::{
    audio::start_audio_capture, lifecycle::RecordingLifecycle, video::start_video_pipeline,
};

/// Recording pipeline manager.
///
/// Orchestrates the capture → encode → buffer data flow.
/// Manages video capture, audio capture, and encoding threads.
///
/// # Thread Safety
///
/// This type is not thread-safe and must be used from a single async context.
pub struct RecordingPipeline {
    /// Handle to the encoder thread.
    encoder_handle: Option<EncoderHandle>,
    /// DXGI capture instance.
    capture: Option<DxgiCapture>,
    /// Audio capture manager.
    audio_manager: Option<crate::capture::audio::WasapiAudioManager>,
    /// Current lifecycle state.
    lifecycle: RecordingLifecycle,
}

impl Default for RecordingPipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl RecordingPipeline {
    /// Creates a new RecordingPipeline in idle state.
    pub fn new() -> Self {
        Self {
            encoder_handle: None,
            capture: None,
            audio_manager: None,
            lifecycle: RecordingLifecycle::Idle,
        }
    }

    /// Gets the current lifecycle state.
    pub fn lifecycle(&self) -> RecordingLifecycle {
        self.lifecycle
    }

    /// Checks if recording is currently active.
    ///
    /// # Returns
    ///
    /// `true` if the pipeline is in the Running state.
    pub fn is_recording(&self) -> bool {
        matches!(self.lifecycle, RecordingLifecycle::Running)
    }

    /// Rolls back startup by cleaning up partially-initialized resources.
    fn rollback_startup(&mut self) {
        if let Some(audio_manager) = self.audio_manager.take() {
            drop(audio_manager);
        }
        if let Some(capture) = self.capture.take() {
            drop(capture);
        }
        if let Some(handle) = self.encoder_handle.take() {
            let crate::encode::EncoderHandle {
                thread,
                frame_tx,
                health_rx: _,
                effective_config: _,
            } = handle;
            drop(frame_tx);
            match thread.join() {
                Ok(Ok(())) => {}
                Ok(Err(e)) => warn!("Encoder thread returned error during rollback: {}", e),
                Err(e) => warn!("Encoder thread panicked during rollback: {:?}", e),
            }
        }
    }

    pub async fn start(&mut self, config: &Config, buffer: &ReplayBuffer) -> Result<()> {
        if matches!(
            self.lifecycle,
            RecordingLifecycle::Starting
                | RecordingLifecycle::Running
                | RecordingLifecycle::Stopping
        ) {
            warn!("Recording already in progress");
            return Ok(());
        }

        info!("Recording: starting pipeline");
        self.lifecycle = RecordingLifecycle::Starting;

        match start_audio_capture(config, buffer, "capture mode") {
            Ok(audio_manager) => {
                if audio_manager.is_running() {
                    self.audio_manager = Some(audio_manager);
                }
            }
            Err(e) => {
                self.rollback_startup();
                self.lifecycle = RecordingLifecycle::Idle;
                return Err(e);
            }
        }

        match start_video_pipeline(config, buffer) {
            Ok((capture, encoder_handle)) => {
                self.capture = Some(capture);
                self.encoder_handle = Some(encoder_handle);
            }
            Err(e) => {
                self.rollback_startup();
                self.lifecycle = RecordingLifecycle::Idle;
                return Err(e);
            }
        }

        self.lifecycle = RecordingLifecycle::Running;
        info!("Recording mode: DXGI capture + native encoder + audio");
        info!("Recording started");
        Ok(())
    }

    pub async fn stop(&mut self) -> Result<()> {
        if matches!(self.lifecycle, RecordingLifecycle::Idle) {
            return Ok(());
        }

        info!("Recording: stopping pipeline");
        self.lifecycle = RecordingLifecycle::Stopping;
        let mut first_error: Option<anyhow::Error> = None;

        if let Some(audio_manager) = self.audio_manager.take() {
            drop(audio_manager);
        }

        if let Some(capture) = self.capture.take() {
            drop(capture);
        }

        if let Some(handle) = self.encoder_handle.take() {
            let crate::encode::EncoderHandle {
                thread,
                frame_tx,
                health_rx: _,
                effective_config: _,
            } = handle;
            drop(frame_tx);

            match thread.join() {
                Ok(Ok(())) => info!("Encoder thread stopped successfully"),
                Ok(Err(e)) => {
                    error!("Encoder thread returned error: {}", e);
                    if first_error.is_none() {
                        first_error = Some(e);
                    }
                }
                Err(_) => {
                    error!("Encoder thread panicked");
                    if first_error.is_none() {
                        first_error = Some(anyhow::anyhow!("Encoder thread panicked"));
                    }
                }
            }
        }

        self.lifecycle = RecordingLifecycle::Idle;
        info!("Recording stopped");
        if let Some(e) = first_error {
            return Err(e);
        }
        Ok(())
    }
    /// Checks and enforces the health of all active pipeline components.
    ///
    /// This method polls for fatal errors from the encoder and capture threads.
    /// If a fatal error is detected, or if a thread has unexpectedly exited,
    /// it transitions the pipeline to the `Faulted` state and performs a clean stop.
    ///
    /// # Returns
    ///
    /// - `Ok(None)` if all components are healthy or if the pipeline is not running.
    /// - `Ok(Some(reason))` if a fatal error was detected and handled.
    ///
    /// # Errors
    ///
    /// Returns an error if attempting to stop the pipeline fails after a fault detection.
    pub async fn enforce_health(&mut self) -> Result<Option<String>> {
        if !matches!(self.lifecycle, RecordingLifecycle::Running) {
            return Ok(None);
        }

        let mut fatal_reason = None;

        if let Some(handle) = self.encoder_handle.as_ref() {
            if let Ok(event) = handle.health_rx.try_recv() {
                match event {
                    crate::encode::EncoderHealthEvent::Fatal(reason) => {
                        fatal_reason = Some(format!("Encoder fatal: {}", reason));
                    }
                }
            } else if handle.thread.is_finished() {
                fatal_reason = Some("Encoder thread exited unexpectedly".to_string());
            }
        }

        if fatal_reason.is_none() {
            if let Some(capture) = self.capture.as_ref() {
                if let Some(reason) = capture.try_recv_fatal() {
                    fatal_reason = Some(format!("Capture fatal: {}", reason));
                } else if capture.is_running() && capture.is_capture_thread_finished() {
                    fatal_reason = Some("Capture thread exited unexpectedly".to_string());
                }
            }
        }

        if let Some(reason) = fatal_reason {
            error!("Fail-closed transition: {}", reason);
            self.lifecycle = RecordingLifecycle::Faulted;
            self.stop().await?;
            return Ok(Some(reason));
        }

        Ok(None)
    }
}

impl Drop for RecordingPipeline {
    fn drop(&mut self) {
        if !matches!(self.lifecycle, RecordingLifecycle::Idle) {
            drop(self.audio_manager.take());
            drop(self.capture.take());
            if let Some(handle) = self.encoder_handle.take() {
                let crate::encode::EncoderHandle {
                    thread,
                    frame_tx,
                    health_rx: _,
                    effective_config: _,
                } = handle;
                drop(frame_tx);

                match thread.join() {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => warn!("Encoder thread returned error during drop: {}", e),
                    Err(e) => warn!("Encoder thread panicked during drop: {:?}", e),
                }
            }
        }
    }
}
