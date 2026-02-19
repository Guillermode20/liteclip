//! Application State Management
//!
//! Central application state coordinating capture, encoding, and buffer management.

use crate::{
    buffer::ring::SharedReplayBuffer,
    capture::{
        audio::WasapiAudioManager, dxgi::DxgiCapture, CaptureBackend, CaptureConfig, CapturedFrame,
    },
    clip::{spawn_clip_saver, MuxerConfig},
    config::Config,
    encode::{spawn_encoder, spawn_encoder_with_receiver, EncoderConfig},
};
use anyhow::{bail, Context, Result};
use crossbeam::channel::Receiver;
use std::path::PathBuf;
use std::time::Duration;
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingLifecycle {
    Idle,
    Starting,
    Running,
    Stopping,
    Faulted,
}

/// Central application state
pub struct AppState {
    config: Config,
    buffer: SharedReplayBuffer,
    encoder_handle: Option<crate::encode::EncoderHandle>,
    capture: Option<DxgiCapture>,
    audio_manager: Option<WasapiAudioManager>,
    lifecycle: RecordingLifecycle,
}

impl AppState {
    fn should_use_hardware_pull_mode(config: &Config) -> bool {
        match config.video.encoder {
            crate::config::EncoderType::Software => false,
            crate::config::EncoderType::Nvenc => {
                if config.advanced.use_cpu_readback {
                    return false;
                }
                crate::encode::hw_encoder::check_encoder_available("h264_nvenc")
            }
            crate::config::EncoderType::Amf => {
                if config.advanced.use_cpu_readback {
                    return false;
                }
                crate::encode::hw_encoder::check_encoder_available("h264_amf")
            }
            crate::config::EncoderType::Qsv => {
                if config.advanced.use_cpu_readback {
                    return false;
                }
                crate::encode::hw_encoder::check_encoder_available("h264_qsv")
            }
            crate::config::EncoderType::Auto => {
                crate::encode::hw_encoder::check_encoder_available("h264_nvenc")
                    || crate::encode::hw_encoder::check_encoder_available("h264_amf")
                    || crate::encode::hw_encoder::check_encoder_available("h264_qsv")
            }
        }
    }

    /// Create new application state with given configuration
    pub fn new(config: Config) -> Result<Self> {
        let buffer = SharedReplayBuffer::new(&config)?;

        Ok(Self {
            config,
            buffer,
            encoder_handle: None,
            capture: None,
            audio_manager: None,
            lifecycle: RecordingLifecycle::Idle,
        })
    }

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
                packet_rx: _,
                health_rx: _,
            } = handle;
            drop(frame_tx);
            match thread.join() {
                Ok(Ok(())) => {}
                Ok(Err(e)) => warn!("Encoder thread returned error during rollback: {}", e),
                Err(e) => warn!("Encoder thread panicked during rollback: {:?}", e),
            }
        }
    }

    /// Start audio capture and spawn forwarding thread
    ///
    /// Extracted to eliminate code duplication between hardware pull mode
    /// and CPU capture mode audio setup.
    fn start_audio_capture(&mut self, context: &str) -> Result<()> {
        if !self.config.audio.capture_system && !self.config.audio.capture_mic {
            return Ok(());
        }

        let mut audio_manager =
            WasapiAudioManager::new().context("Failed to create audio manager")?;
        audio_manager
            .start(&self.config.audio)
            .context("Failed to start audio capture")?;

        let audio_packet_rx = audio_manager.packet_rx();
        let buffer_clone = self.buffer.clone();
        let context_label = context.to_string();
        let context_for_thread = context_label.clone();

        std::thread::spawn(move || {
            let mut forwarded_packets = 0u64;
            while let Ok(packet) = audio_packet_rx.recv() {
                buffer_clone.push(packet);
                forwarded_packets = forwarded_packets.saturating_add(1);

                if forwarded_packets == 1 {
                    debug!(
                        "Forwarded first audio packet to replay buffer ({})",
                        context_for_thread
                    );
                } else if forwarded_packets % 500 == 0 {
                    debug!(
                        "Forwarded {} audio packets to replay buffer",
                        forwarded_packets
                    );
                }
            }
        });

        self.audio_manager = Some(audio_manager);
        info!("Audio capture started ({})", context_label);
        Ok(())
    }

    /// Start the recording pipeline
    pub async fn start_recording(&mut self) -> Result<()> {
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

        let mut encoder_config = EncoderConfig::from(&self.config);

        if Self::should_use_hardware_pull_mode(&self.config) {
            encoder_config.use_cpu_readback = false;
            info!("Recording mode: hardware pull (FFmpeg desktop grab)");
            let (encoder_handle, _unused_frame_tx) =
                match spawn_encoder(encoder_config, self.buffer.clone())
                    .context("Failed to spawn pull-mode encoder")
                {
                    Ok(v) => v,
                    Err(e) => {
                        self.rollback_startup();
                        self.lifecycle = RecordingLifecycle::Idle;
                        return Err(e);
                    }
                };

            self.encoder_handle = Some(encoder_handle);

            if let Err(e) = self.start_audio_capture("pull mode") {
                self.rollback_startup();
                self.lifecycle = RecordingLifecycle::Idle;
                return Err(e);
            }

            self.capture = None;
            self.lifecycle = RecordingLifecycle::Running;
            info!("Recording started");
            return Ok(());
        }

        // Create and start audio capture first if enabled
        if let Err(e) = self.start_audio_capture("capture mode") {
            self.rollback_startup();
            self.lifecycle = RecordingLifecycle::Idle;
            return Err(e);
        }

        // Create and start video capture
        let mut capture = match DxgiCapture::new().context("Failed to create DXGI capture") {
            Ok(capture) => capture,
            Err(e) => {
                self.rollback_startup();
                self.lifecycle = RecordingLifecycle::Idle;
                return Err(e);
            }
        };
        let mut capture_config = CaptureConfig::from(&self.config);
        capture_config.perform_cpu_readback = true;

        if let Err(e) = capture
            .start(capture_config)
            .context("Failed to start capture")
        {
            self.rollback_startup();
            self.lifecycle = RecordingLifecycle::Idle;
            return Err(e);
        }

        // Get the frame receiver from capture
        let frame_rx: Receiver<CapturedFrame> = capture.frame_rx();

        // Initialize encoder with the capture's frame receiver
        let mut capture_encoder_config = encoder_config;
        // This pipeline provides captured frames via frame_rx, so encoder must
        // consume pushed BGRA frames (not FFmpeg desktop pull mode).
        capture_encoder_config.use_cpu_readback = true;

        let encoder_handle = match spawn_encoder_with_receiver(
            capture_encoder_config,
            self.buffer.clone(),
            frame_rx,
        )
        .context("Failed to spawn encoder")
        {
            Ok(handle) => handle,
            Err(e) => {
                drop(capture);
                self.rollback_startup();
                self.lifecycle = RecordingLifecycle::Idle;
                return Err(e);
            }
        };

        self.encoder_handle = Some(encoder_handle);
        self.capture = Some(capture);
        self.lifecycle = RecordingLifecycle::Running;

        info!("Recording mode: capture + encoder + audio");
        info!("Recording started");

        Ok(())
    }

    /// Stop the recording pipeline
    pub async fn stop_recording(&mut self) -> Result<()> {
        if matches!(self.lifecycle, RecordingLifecycle::Idle) {
            return Ok(());
        }

        info!("Recording: stopping pipeline");
        self.lifecycle = RecordingLifecycle::Stopping;
        let mut first_error: Option<anyhow::Error> = None;

        // Stop audio capture first
        if let Some(audio_manager) = self.audio_manager.take() {
            drop(audio_manager); // This calls stop() via Drop
            debug!("Audio capture stopped");
        }

        // Stop video capture (signals encoder to stop via channel close)
        if let Some(capture) = self.capture.take() {
            drop(capture); // This calls stop() via Drop
            debug!("Video capture stopped");
        }

        // Wait for encoder thread to finish
        if let Some(handle) = self.encoder_handle.take() {
            let crate::encode::EncoderHandle {
                thread,
                frame_tx,
                packet_rx: _,
                health_rx: _,
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
                Err(e) => {
                    error!("Encoder thread panicked: {:?}", e);
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

    pub async fn enforce_pipeline_health(&mut self) -> Result<Option<String>> {
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
            self.stop_recording().await?;
            return Ok(Some(reason));
        }

        Ok(None)
    }

    /// Save the current buffer contents to a clip
    pub async fn save_clip(&self) -> Result<PathBuf> {
        info!("Clip: saving replay buffer");

        let output_path = self.generate_output_path()?;

        let stats = self.buffer.stats();
        info!(
            "Buffer stats before save: {} packets, {} bytes, {} keyframes",
            stats.packet_count, stats.total_bytes, stats.keyframe_count
        );

        if stats.packet_count == 0 {
            warn!("Buffer is empty - cannot save clip");
            bail!("Buffer is empty - no frames to save");
        }

        if stats.keyframe_count == 0 {
            warn!("No keyframe in buffer - cannot save clip yet");
            bail!(
                "No keyframe available - please wait a moment for the next keyframe before saving"
            );
        }

        let (width, height) = self
            .buffer
            .snapshot_first_packet_resolution()
            .unwrap_or(match self.config.video.resolution {
                crate::config::Resolution::Native => (1920, 1080),
                crate::config::Resolution::P1080 => (1920, 1080),
                crate::config::Resolution::P720 => (1280, 720),
                crate::config::Resolution::P480 => (854, 480),
            });
        let fps = self.config.video.framerate as f64;

        let muxer_video_codec = match self.config.video.codec {
            crate::config::Codec::H264 => "h264",
            crate::config::Codec::H265 => "hevc",
            crate::config::Codec::Av1 => "av1",
        };
        let muxer_config = MuxerConfig::new(width, height, fps, &output_path)
            .with_video_codec(muxer_video_codec)
            .with_expect_audio(self.config.audio.capture_system || self.config.audio.capture_mic);

        let buffer = self.buffer.clone();
        let duration = Duration::from_secs(self.config.general.replay_duration_secs as u64);

        info!("Spawning clip saver task...");
        let handle = spawn_clip_saver(buffer, duration, output_path.clone(), muxer_config);

        info!("Waiting for clip saver task to complete...");
        let result = handle.await.context("Clip saver task panicked")?;
        let final_path = result?;

        info!("Clip saver completed (buffer preserved for continuous replay)");

        Ok(final_path)
    }

    /// Generate output path for a new clip
    fn generate_output_path(&self) -> Result<PathBuf> {
        use chrono::Local;

        let timestamp = Local::now().format("%Y-%m-%d_%H-%M-%S");
        let filename = format!("clip_{}.mp4", timestamp);

        let save_dir = PathBuf::from(&self.config.general.save_directory);
        std::fs::create_dir_all(&save_dir)?;

        Ok(save_dir.join(filename))
    }

    /// Get current buffer stats
    pub fn buffer_stats(&self) -> crate::buffer::BufferStats {
        self.buffer.stats()
    }

    /// Check if currently recording
    pub fn is_recording(&self) -> bool {
        matches!(self.lifecycle, RecordingLifecycle::Running)
    }

    /// Current recording lifecycle state
    pub fn lifecycle(&self) -> RecordingLifecycle {
        self.lifecycle
    }

    /// Handle hotkey action
    pub fn handle_hotkey(&mut self, action: crate::platform::HotkeyAction) {
        match action {
            crate::platform::HotkeyAction::SaveClip => {
                info!("Hotkey: SaveClip");
                // Note: This is called from sync context, use try_ variants
                // The actual save is handled in the async event loop
            }
            crate::platform::HotkeyAction::ToggleRecording => {
                info!("Hotkey: ToggleRecording");
            }
            _ => {}
        }
    }

    /// Apply configuration changes that don't require restart
    ///
    /// Updates runtime-modifiable settings like audio volumes and replay buffer duration.
    /// Logs all changes at info level for visibility.
    ///
    /// # Arguments
    /// * `new_config` - The new configuration to apply
    ///
    /// # Returns
    /// * `Ok(())` if changes were applied successfully
    /// * `Err` if there was an error applying changes
    pub fn apply_runtime_config(&mut self, new_config: &Config) -> Result<()> {
        info!("Applying runtime configuration changes...");

        // Update audio settings if audio manager is active
        if self.audio_manager.is_some() {
            // Check if system volume changed
            if self.config.audio.system_volume != new_config.audio.system_volume {
                info!(
                    "Audio: System volume changed from {}% to {}%",
                    self.config.audio.system_volume, new_config.audio.system_volume
                );
            }

            // Check if mic volume changed
            if self.config.audio.mic_volume != new_config.audio.mic_volume {
                info!(
                    "Audio: Mic volume changed from {}% to {}%",
                    self.config.audio.mic_volume, new_config.audio.mic_volume
                );
            }

            // Check if audio capture settings changed (would require restart)
            if self.config.audio.capture_system != new_config.audio.capture_system {
                warn!(
                    "Audio: System capture toggle changed ({} -> {}), requires restart",
                    self.config.audio.capture_system, new_config.audio.capture_system
                );
            }

            if self.config.audio.capture_mic != new_config.audio.capture_mic {
                warn!(
                    "Audio: Mic capture toggle changed ({} -> {}), requires restart",
                    self.config.audio.capture_mic, new_config.audio.capture_mic
                );
            }
        }

        // Check replay duration changes
        if self.config.general.replay_duration_secs != new_config.general.replay_duration_secs {
            info!(
                "Buffer: Replay duration changed from {}s to {}s (effective on next buffer creation)",
                self.config.general.replay_duration_secs, new_config.general.replay_duration_secs
            );
            // Note: Existing replay buffer instances keep their original duration.
            // This takes effect after buffer recreation (e.g., app restart).
        }

        // Update the stored configuration
        self.config = new_config.clone();

        info!("Runtime configuration changes applied successfully");
        Ok(())
    }

    /// Get a reference to the current configuration
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Get a mutable reference to the current configuration
    pub fn config_mut(&mut self) -> &mut Config {
        &mut self.config
    }
}

impl Drop for AppState {
    fn drop(&mut self) {
        if !matches!(self.lifecycle, RecordingLifecycle::Idle) {
            // Clean shutdown attempt in blocking context
            drop(self.audio_manager.take());
            drop(self.capture.take());
            if let Some(handle) = self.encoder_handle.take() {
                let crate::encode::EncoderHandle {
                    thread,
                    frame_tx,
                    packet_rx: _,
                    health_rx: _,
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
