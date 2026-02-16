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
use anyhow::{Context, Result};
use crossbeam::channel::Receiver;
use std::path::PathBuf;
use std::time::Duration;
use tracing::{debug, error, info, warn};

/// Central application state
pub struct AppState {
    config: Config,
    buffer: SharedReplayBuffer,
    encoder_handle: Option<crate::encode::EncoderHandle>,
    capture: Option<DxgiCapture>,
    audio_manager: Option<WasapiAudioManager>,
    is_recording: bool,
}

impl AppState {
    fn should_use_hardware_pull_mode(config: &Config) -> bool {
        if config.advanced.use_cpu_readback {
            return false;
        }

        match config.video.encoder {
            crate::config::EncoderType::Nvenc => {
                crate::encode::hw_encoder::check_encoder_available("h264_nvenc")
            }
            crate::config::EncoderType::Amf => {
                crate::encode::hw_encoder::check_encoder_available("h264_amf")
            }
            crate::config::EncoderType::Qsv => {
                crate::encode::hw_encoder::check_encoder_available("h264_qsv")
            }
            _ => false,
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
            is_recording: false,
        })
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
        let context = context.to_string();

        std::thread::spawn(move || {
            let mut forwarded_packets = 0u64;
            while let Ok(packet) = audio_packet_rx.recv() {
                buffer_clone.push(packet);
                forwarded_packets = forwarded_packets.saturating_add(1);

                if forwarded_packets == 1 {
                    debug!(
                        "Forwarded first audio packet to replay buffer ({})",
                        context
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
        debug!("Audio capture started");
        Ok(())
    }

    /// Start the recording pipeline
    pub async fn start_recording(&mut self) -> Result<()> {
        if self.is_recording {
            warn!("Recording already in progress");
            return Ok(());
        }

        info!("Starting recording pipeline...");

        let encoder_config = EncoderConfig::from(&self.config);

        if Self::should_use_hardware_pull_mode(&self.config) {
            info!("Using hardware pull mode (FFmpeg desktop grab) - skipping app CPU readback capture");
            let (encoder_handle, _unused_frame_tx) =
                spawn_encoder(encoder_config, self.buffer.clone())
                    .context("Failed to spawn pull-mode encoder")?;

            self.start_audio_capture("pull mode")?;

            self.encoder_handle = Some(encoder_handle);
            self.capture = None;
            self.is_recording = true;
            info!("Recording pipeline started (encoder pull mode)");
            return Ok(());
        }

        // Create and start audio capture first if enabled
        self.start_audio_capture("capture mode")?;

        // Create and start video capture
        let mut capture = DxgiCapture::new().context("Failed to create DXGI capture")?;
        let capture_config = CaptureConfig::from(&self.config);

        capture
            .start(capture_config)
            .context("Failed to start capture")?;

        // Get the frame receiver from capture
        let frame_rx: Receiver<CapturedFrame> = capture.frame_rx();

        // Initialize encoder with the capture's frame receiver
        let mut capture_encoder_config = encoder_config;
        // This pipeline provides captured frames via frame_rx, so encoder must
        // consume pushed BGRA frames (not FFmpeg desktop pull mode).
        capture_encoder_config.use_cpu_readback = true;

        let encoder_handle =
            spawn_encoder_with_receiver(capture_encoder_config, self.buffer.clone(), frame_rx)
                .context("Failed to spawn encoder")?;

        self.encoder_handle = Some(encoder_handle);
        self.capture = Some(capture);
        self.is_recording = true;

        info!("Recording pipeline started (capture + encoder + audio)");

        Ok(())
    }

    /// Stop the recording pipeline
    pub async fn stop_recording(&mut self) -> Result<()> {
        if !self.is_recording {
            return Ok(());
        }

        info!("Stopping recording pipeline...");

        // Stop audio capture first
        if let Some(audio_manager) = self.audio_manager.take() {
            drop(audio_manager); // This calls stop() via Drop
            info!("Audio capture stopped");
        }

        // Stop video capture (signals encoder to stop via channel close)
        if let Some(capture) = self.capture.take() {
            drop(capture); // This calls stop() via Drop
            info!("Video capture stopped");
        }

        // Wait for encoder thread to finish
        if let Some(handle) = self.encoder_handle.take() {
            let crate::encode::EncoderHandle {
                thread,
                frame_tx,
                packet_rx: _,
            } = handle;
            drop(frame_tx);

            match thread.join() {
                Ok(_) => info!("Encoder thread stopped successfully"),
                Err(e) => error!("Encoder thread panicked: {:?}", e),
            }
        }

        self.is_recording = false;
        info!("Recording pipeline stopped");
        Ok(())
    }

    /// Save the current buffer contents to a clip
    pub async fn save_clip(&self) -> Result<PathBuf> {
        info!("Saving clip...");

        let output_path = self.generate_output_path()?;

        // Get resolution from the first packet in buffer, or fall back to config
        let (width, height) = self
            .buffer
            .snapshot_first_packet_resolution()
            .unwrap_or(match self.config.video.resolution {
                crate::config::Resolution::Native => (1920, 1080), // Fallback if no packets
                crate::config::Resolution::P1080 => (1920, 1080),
                crate::config::Resolution::P720 => (1280, 720),
                crate::config::Resolution::P480 => (854, 480),
            });
        let fps = self.config.video.framerate as f64;

        let muxer_config = MuxerConfig::new(width, height, fps, &output_path)
            .with_expect_audio(self.config.audio.capture_system || self.config.audio.capture_mic);

        // Clone buffer for the clip saver task
        let buffer = self.buffer.clone();
        let duration = Duration::from_secs(self.config.general.replay_duration_secs as u64);

        let handle = spawn_clip_saver(buffer, duration, output_path.clone(), muxer_config);

        let result = handle.await.context("Clip saver task panicked")?;
        let final_path = result?;

        debug!("Clip saved to: {:?}", final_path);
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
        self.is_recording
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
}

impl Drop for AppState {
    fn drop(&mut self) {
        if self.is_recording {
            // Clean shutdown attempt in blocking context
            drop(self.audio_manager.take());
            drop(self.capture.take());
            if let Some(handle) = self.encoder_handle.take() {
                let crate::encode::EncoderHandle {
                    thread,
                    frame_tx,
                    packet_rx: _,
                } = handle;
                drop(frame_tx);

                match thread.join() {
                    Ok(_) => {}
                    Err(e) => warn!("Encoder thread panicked during drop: {:?}", e),
                }
            }
        }
    }
}
