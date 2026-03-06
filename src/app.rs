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
    encode::{spawn_encoder, spawn_encoder_with_receiver, EncoderConfig, EncoderHandle},
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

/// Manages the recording pipeline (video/audio capture and encoding)
pub struct RecordingPipeline {
    encoder_handle: Option<EncoderHandle>,
    capture: Option<DxgiCapture>,
    audio_manager: Option<WasapiAudioManager>,
    lifecycle: RecordingLifecycle,
}

impl Default for RecordingPipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl RecordingPipeline {
    pub fn new() -> Self {
        Self {
            encoder_handle: None,
            capture: None,
            audio_manager: None,
            lifecycle: RecordingLifecycle::Idle,
        }
    }

    pub fn lifecycle(&self) -> RecordingLifecycle {
        self.lifecycle
    }

    pub fn is_recording(&self) -> bool {
        matches!(self.lifecycle, RecordingLifecycle::Running)
    }

    fn should_use_hardware_pull_mode(_config: &Config) -> bool {
        // We are now using native FFmpeg integration which expects frames from DXGI capture.
        // The old hardware pull mode (CLI gdigrab) is obsolete.
        false
    }

    fn rollback_startup(&mut self) {
        if let Some(audio_manager) = self.audio_manager.take() {
            drop(audio_manager);
        }
        if let Some(capture) = self.capture.take() {
            drop(capture);
        }
        if let Some(handle) = self.encoder_handle.take() {
            let EncoderHandle {
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

    fn start_audio_capture(
        &mut self,
        config: &Config,
        buffer: &SharedReplayBuffer,
        context: &str,
    ) -> Result<()> {
        if !config.audio.capture_system && !config.audio.capture_mic {
            return Ok(());
        }

        let mut audio_manager =
            WasapiAudioManager::new().context("Failed to create audio manager")?;
        audio_manager
            .start(&config.audio)
            .context("Failed to start audio capture")?;

        let audio_packet_rx = audio_manager.packet_rx();
        let buffer_clone = buffer.clone();
        let context_label = context.to_string();
        let context_for_thread = context_label.clone();

        std::thread::spawn(move || {
            let mut forwarded_packets = 0u64;
            let mut packet_batch = Vec::with_capacity(32);

            while let Ok(packet) = audio_packet_rx.recv() {
                packet_batch.push(packet);
                forwarded_packets = forwarded_packets.saturating_add(1);

                while packet_batch.len() < 32 {
                    if let Ok(p) = audio_packet_rx.try_recv() {
                        packet_batch.push(p);
                        forwarded_packets = forwarded_packets.saturating_add(1);
                    } else {
                        break;
                    }
                }

                buffer_clone.push_batch(packet_batch.drain(..));

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
        });

        self.audio_manager = Some(audio_manager);
        info!("Audio capture started ({})", context_label);
        Ok(())
    }

    pub async fn start(&mut self, config: &Config, buffer: &SharedReplayBuffer) -> Result<()> {
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

        let mut encoder_config = EncoderConfig::from(config);

        if Self::should_use_hardware_pull_mode(config) {
            encoder_config.use_cpu_readback = false;
            info!("Recording mode: hardware pull (FFmpeg desktop grab)");
            let (encoder_handle, _unused_frame_tx) =
                match spawn_encoder(encoder_config, buffer.clone())
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

            if let Err(e) = self.start_audio_capture(config, buffer, "pull mode") {
                self.rollback_startup();
                self.lifecycle = RecordingLifecycle::Idle;
                return Err(e);
            }

            self.capture = None;
            self.lifecycle = RecordingLifecycle::Running;
            info!("Recording started");
            return Ok(());
        }

        if let Err(e) = self.start_audio_capture(config, buffer, "capture mode") {
            self.rollback_startup();
            self.lifecycle = RecordingLifecycle::Idle;
            return Err(e);
        }

        let mut capture = match DxgiCapture::new().context("Failed to create DXGI capture") {
            Ok(capture) => capture,
            Err(e) => {
                self.rollback_startup();
                self.lifecycle = RecordingLifecycle::Idle;
                return Err(e);
            }
        };
        let mut capture_config = CaptureConfig::from(config);
        capture_config.perform_cpu_readback = true;

        if let Err(e) = capture
            .start(capture_config)
            .context("Failed to start capture")
        {
            self.rollback_startup();
            self.lifecycle = RecordingLifecycle::Idle;
            return Err(e);
        }

        let frame_rx: Receiver<CapturedFrame> = capture.frame_rx();

        let mut capture_encoder_config = encoder_config;
        capture_encoder_config.use_cpu_readback = true;

        let encoder_handle =
            match spawn_encoder_with_receiver(capture_encoder_config, buffer.clone(), frame_rx)
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

    pub async fn stop(&mut self) -> Result<()> {
        if matches!(self.lifecycle, RecordingLifecycle::Idle) {
            return Ok(());
        }

        info!("Recording: stopping pipeline");
        self.lifecycle = RecordingLifecycle::Stopping;
        let mut first_error: Option<anyhow::Error> = None;

        if let Some(audio_manager) = self.audio_manager.take() {
            drop(audio_manager);
            debug!("Audio capture stopped");
        }

        if let Some(capture) = self.capture.take() {
            drop(capture);
            debug!("Video capture stopped");
        }

        if let Some(handle) = self.encoder_handle.take() {
            let EncoderHandle {
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
                let EncoderHandle {
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

/// Manages clip saving operations
pub struct ClipManager;

impl ClipManager {
    pub async fn save_clip(config: &Config, buffer: &SharedReplayBuffer) -> Result<PathBuf> {
        info!("Clip: saving replay buffer");

        let output_path = Self::generate_output_path(config)?;

        let stats = buffer.stats();
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

        let (width, height) =
            buffer
                .snapshot_first_packet_resolution()
                .unwrap_or(match config.video.resolution {
                    crate::config::Resolution::Native => (1920, 1080),
                    crate::config::Resolution::P1080 => (1920, 1080),
                    crate::config::Resolution::P720 => (1280, 720),
                    crate::config::Resolution::P480 => (854, 480),
                });
        let fps = config.video.framerate as f64;

        let muxer_video_codec = match config.video.codec {
            crate::config::Codec::H264 => "h264",
            crate::config::Codec::H265 => "hevc",
            crate::config::Codec::Av1 => "av1",
        };
        let muxer_config = MuxerConfig::new(width, height, fps, &output_path)
            .with_video_codec(muxer_video_codec)
            .with_expect_audio(config.audio.capture_system || config.audio.capture_mic);

        let buffer_clone = buffer.clone();
        let duration = Duration::from_secs(config.general.replay_duration_secs as u64);

        info!("Spawning clip saver task...");
        let handle = spawn_clip_saver(buffer_clone, duration, output_path.clone(), muxer_config);

        info!("Waiting for clip saver task to complete...");
        let result = handle.await.context("Clip saver task panicked")?;
        let final_path = result?;

        info!("Clip saver completed (buffer preserved for continuous replay)");

        Ok(final_path)
    }

    fn generate_output_path(config: &Config) -> Result<PathBuf> {
        use chrono::Local;

        let timestamp = Local::now().format("%Y-%m-%d_%H-%M-%S_%3f");
        let filename = format!("clip_{}.mp4", timestamp);

        let save_dir = PathBuf::from(&config.general.save_directory);
        std::fs::create_dir_all(&save_dir)?;

        Ok(save_dir.join(filename))
    }
}

/// Central application state
pub struct AppState {
    config: Config,
    buffer: SharedReplayBuffer,
    pipeline: RecordingPipeline,
}

impl AppState {
    pub fn new(config: Config) -> Result<Self> {
        let buffer = SharedReplayBuffer::new(&config)?;

        Ok(Self {
            config,
            buffer,
            pipeline: RecordingPipeline::new(),
        })
    }

    pub async fn start_recording(&mut self) -> Result<()> {
        self.pipeline.start(&self.config, &self.buffer).await
    }

    pub async fn stop_recording(&mut self) -> Result<()> {
        self.pipeline.stop().await
    }

    pub async fn enforce_pipeline_health(&mut self) -> Result<Option<String>> {
        self.pipeline.enforce_health().await
    }

    pub async fn save_clip(&self) -> Result<PathBuf> {
        ClipManager::save_clip(&self.config, &self.buffer).await
    }

    pub fn save_context(&self) -> (Config, SharedReplayBuffer, bool) {
        (
            self.config.clone(),
            self.buffer.clone(),
            self.config.general.notifications,
        )
    }

    pub fn buffer_stats(&self) -> crate::buffer::BufferStats {
        self.buffer.stats()
    }

    pub fn is_recording(&self) -> bool {
        self.pipeline.is_recording()
    }

    pub fn lifecycle(&self) -> RecordingLifecycle {
        self.pipeline.lifecycle()
    }

    pub fn handle_hotkey(&mut self, action: crate::platform::HotkeyAction) {
        match action {
            crate::platform::HotkeyAction::SaveClip => {
                info!("Hotkey: SaveClip");
            }
            crate::platform::HotkeyAction::ToggleRecording => {
                info!("Hotkey: ToggleRecording");
            }
            _ => {}
        }
    }

    pub fn apply_runtime_config(&mut self, new_config: &Config) -> Result<()> {
        info!("Applying runtime configuration changes...");

        // Log audio changes (cannot easily check active status since it's hidden in pipeline, but we can just log config differences)
        if self.config.audio.system_volume != new_config.audio.system_volume {
            info!(
                "Audio: System volume changed from {}% to {}%",
                self.config.audio.system_volume, new_config.audio.system_volume
            );
        }

        if self.config.audio.mic_volume != new_config.audio.mic_volume {
            info!(
                "Audio: Mic volume changed from {}% to {}%",
                self.config.audio.mic_volume, new_config.audio.mic_volume
            );
        }

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

        if self.config.general.replay_duration_secs != new_config.general.replay_duration_secs {
            info!(
                "Buffer: Replay duration changed from {}s to {}s (effective on next buffer creation)",
                self.config.general.replay_duration_secs, new_config.general.replay_duration_secs
            );
        }

        self.config = new_config.clone();

        info!("Runtime configuration changes applied successfully");
        Ok(())
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn config_mut(&mut self) -> &mut Config {
        &mut self.config
    }
}
