//! WASAPI Audio Manager
//!
//! Coordinates system and microphone audio capture with mixing.

use anyhow::Result;
use crossbeam::channel::Receiver;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::info;

use crate::capture::audio::mic::WasapiMicConfig;
use crate::capture::audio::mixer::AudioMixerConfig;
use crate::capture::audio::system::WasapiSystemConfig;
use crate::capture::audio::{AudioMixer, WasapiMicCapture, WasapiSystemCapture};
use crate::config::AudioConfig;
use crate::encode::EncodedPacket;

/// WASAPI audio manager that coordinates system and microphone capture
pub struct WasapiAudioManager {
    system_capture: Option<WasapiSystemCapture>,
    mic_capture: Option<WasapiMicCapture>,
    mixer: Option<AudioMixer>,
    running: Arc<AtomicBool>,
    packet_rx: Receiver<EncodedPacket>,
}

impl WasapiAudioManager {
    /// Create a new WASAPI audio manager
    pub fn new() -> Result<Self> {
        let (_, packet_rx) = crossbeam::channel::bounded(128); // Placeholder, will be replaced

        Ok(Self {
            system_capture: None,
            mic_capture: None,
            mixer: None,
            running: Arc::new(AtomicBool::new(false)),
            packet_rx,
        })
    }

    /// Start audio capture based on configuration
    pub fn start(&mut self, config: &AudioConfig) -> Result<()> {
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        info!("Starting WASAPI audio manager...");

        // Create system audio capture if enabled
        if config.capture_system {
            let mut system_capture = WasapiSystemCapture::new()?;
            let system_config = WasapiSystemConfig {
                sample_rate: 48000,
                channels: 2,
                bits_per_sample: 16,
                buffer_duration: std::time::Duration::from_millis(100),
                device_id: None, // Use default device
            };

            system_capture.start(system_config)?;
            self.system_capture = Some(system_capture);
            info!("System audio capture started");
        }

        // Create microphone capture if enabled
        if config.capture_mic {
            let mut mic_capture = WasapiMicCapture::new()?;
            let mic_config = WasapiMicConfig {
                sample_rate: 48000,
                channels: 2,
                bits_per_sample: 16,
                buffer_duration: std::time::Duration::from_millis(100),
                device_id: if config.mic_device == "default" {
                    None
                } else {
                    Some(config.mic_device.clone())
                },
            };

            mic_capture.start(mic_config)?;
            self.mic_capture = Some(mic_capture);
            info!("Microphone audio capture started");
        }

        // Create and configure mixer
        let mut mixer = AudioMixer::new()?;

        // Set up receivers based on what's enabled
        if let Some(ref system_capture) = self.system_capture {
            mixer.set_system_rx(system_capture.packet_rx());
        }

        if let Some(ref mic_capture) = self.mic_capture {
            mixer.set_mic_rx(mic_capture.packet_rx());
        }

        let mixer_config = AudioMixerConfig {
            system_volume: (config.system_volume as f32) / 100.0,
            mic_volume: (config.mic_volume as f32) / 100.0,
            ..Default::default()
        };

        // Store the mixer's output receiver
        let packet_rx = mixer.packet_rx();
        mixer.start(mixer_config)?;
        self.mixer = Some(mixer);
        self.packet_rx = packet_rx;

        self.running.store(true, Ordering::SeqCst);
        info!("WASAPI audio manager started");

        Ok(())
    }

    /// Stop audio capture
    pub fn stop(&mut self) {
        if !self.running.load(Ordering::SeqCst) {
            return;
        }

        info!("Stopping WASAPI audio manager...");

        if let Some(mut system_capture) = self.system_capture.take() {
            system_capture.stop();
        }

        if let Some(mut mic_capture) = self.mic_capture.take() {
            mic_capture.stop();
        }

        if let Some(mut mixer) = self.mixer.take() {
            mixer.stop();
        }

        self.running.store(false, Ordering::SeqCst);
        info!("WASAPI audio manager stopped");
    }

    /// Get receiver for mixed audio packets
    pub fn packet_rx(&self) -> Receiver<EncodedPacket> {
        self.packet_rx.clone()
    }

    /// Check if audio capture is running
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

impl Drop for WasapiAudioManager {
    fn drop(&mut self) {
        self.stop();
    }
}
