//! WASAPI Audio Manager
//!
//! Coordinates system and microphone audio capture with mixing.

use anyhow::Result;

use crossbeam::channel::{Receiver, Sender, TryRecvError};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tracing::{debug, error, warn};

use crate::capture::audio::level_monitor::{calculate_levels_stereo, AudioLevelMonitor};
use crate::capture::audio::mic::WasapiMicConfig;
use crate::capture::audio::mixer::AudioMixer;
use crate::capture::audio::system::WasapiSystemConfig;
use crate::capture::audio::{WasapiMicCapture, WasapiSystemCapture};
use crate::config::AudioConfig;
use crate::encode::EncodedPacket;

/// WASAPI audio manager that coordinates system and microphone capture
pub struct WasapiAudioManager {
    system_capture: Option<WasapiSystemCapture>,
    mic_capture: Option<WasapiMicCapture>,
    running: Arc<AtomicBool>,
    packet_tx: Sender<EncodedPacket>,
    packet_rx: Receiver<EncodedPacket>,
    forward_thread: Option<thread::JoinHandle<()>>,
    config_update_tx: Sender<AudioConfig>,
    config_update_rx: Receiver<AudioConfig>,
    level_monitor: Option<AudioLevelMonitor>,
}

impl WasapiAudioManager {
    /// Create a new WASAPI audio manager
    pub fn new() -> Result<Self> {
        Self::with_level_monitor(None)
    }

    /// Create a new WASAPI audio manager with an optional level monitor
    pub fn with_level_monitor(level_monitor: Option<AudioLevelMonitor>) -> Result<Self> {
        let (packet_tx, packet_rx) = crossbeam::channel::bounded(64);
        let (config_update_tx, config_update_rx) = crossbeam::channel::bounded(16);

        Ok(Self {
            system_capture: None,
            mic_capture: None,
            running: Arc::new(AtomicBool::new(false)),
            packet_tx,
            packet_rx,
            forward_thread: None,
            config_update_tx,
            config_update_rx,
            level_monitor,
        })
    }

    /// Start audio capture based on configuration
    pub fn start(&mut self, config: &AudioConfig) -> Result<()> {
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        debug!("Starting WASAPI audio manager...");
        const CAPTURE_BUFFER_DURATION: Duration = Duration::from_millis(20);

        // Create system audio capture if enabled
        if config.capture_system {
            let mut system_capture = WasapiSystemCapture::new()?;
            let system_config = WasapiSystemConfig {
                sample_rate: 48000,
                channels: 2,
                bits_per_sample: 16,
                buffer_duration: CAPTURE_BUFFER_DURATION,
                device_id: None, // Use default device
            };

            if let Err(e) = system_capture.start(system_config) {
                self.running.store(false, Ordering::SeqCst);
                return Err(e);
            }
            self.system_capture = Some(system_capture);
            debug!("System audio capture started");
        }

        // Create microphone capture if enabled
        if config.capture_mic {
            let mut mic_capture = WasapiMicCapture::new()?;
            let mic_config = WasapiMicConfig {
                sample_rate: 48000,
                channels: 2,
                bits_per_sample: 16,
                buffer_duration: CAPTURE_BUFFER_DURATION,
                device_id: if config.mic_device == "default" {
                    None
                } else {
                    Some(config.mic_device.clone())
                },
                noise_reduction_enabled: config.mic_noise_reduction,
            };

            if let Err(e) = mic_capture.start(mic_config) {
                if let Some(mut system_capture) = self.system_capture.take() {
                    system_capture.stop();
                }
                self.running.store(false, Ordering::SeqCst);
                return Err(e);
            }
            self.mic_capture = Some(mic_capture);
            debug!("Microphone audio capture started");
        }

        let mut system_rx = self
            .system_capture
            .as_ref()
            .map(|capture| capture.packet_rx());
        let mut mic_rx = self.mic_capture.as_ref().map(|capture| capture.receiver());

        self.running.store(true, Ordering::SeqCst);

        let running = Arc::clone(&self.running);
        let packet_tx = self.packet_tx.clone();
        let config_clone = config.clone();
        let config_update_rx = self.config_update_rx.clone();
        let level_monitor = self.level_monitor.clone();
        self.forward_thread = Some(thread::spawn(move || {
            Self::forward_loop(
                running,
                packet_tx,
                &mut system_rx,
                &mut mic_rx,
                config_clone,
                config_update_rx,
                level_monitor,
            )
        }));

        debug!("WASAPI audio manager started");

        Ok(())
    }

    /// Stop audio capture
    pub fn stop(&mut self) {
        if !self.running.load(Ordering::SeqCst) {
            return;
        }

        self.running.store(false, Ordering::SeqCst);
        debug!("Stopping WASAPI audio manager...");

        if let Some(mut system_capture) = self.system_capture.take() {
            system_capture.stop();
        }

        if let Some(mut mic_capture) = self.mic_capture.take() {
            mic_capture.stop();
        }

        if let Some(thread) = self.forward_thread.take() {
            if thread.join().is_err() {
                error!("Audio forward thread panicked");
            }
        }

        debug!("WASAPI audio manager stopped");
    }

    /// Get receiver for mixed audio packets
    pub fn packet_rx(&self) -> Receiver<EncodedPacket> {
        self.packet_rx.clone()
    }

    /// Check if audio capture is running
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Update audio configuration at runtime
    ///
    /// This allows changing volume levels and other settings without restarting capture.
    pub fn update_config(&self, config: &AudioConfig) {
        if self.running.load(Ordering::SeqCst) {
            if let Err(e) = self.config_update_tx.try_send(config.clone()) {
                warn!("Failed to send config update to audio manager: {}", e);
            }
        }
    }

    fn forward_loop(
        running: Arc<AtomicBool>,
        packet_tx: Sender<EncodedPacket>,
        system_rx: &mut Option<Receiver<EncodedPacket>>,
        mic_rx: &mut Option<Receiver<EncodedPacket>>,
        config: AudioConfig,
        config_update_rx: Receiver<AudioConfig>,
        level_monitor: Option<AudioLevelMonitor>,
    ) {
        const PACKET_PAIR_WAIT: Duration = Duration::from_millis(25);
        let mut mixer = AudioMixer::new(&config);
        let mut system_packet: Option<EncodedPacket> = None;
        let mut mic_packet: Option<EncodedPacket> = None;
        let mut system_packet_started_at: Option<Instant> = None;
        let mut mic_packet_started_at: Option<Instant> = None;
        let mut forwarded_total: u64 = 0;
        let mut last_peak_decay = Instant::now();

        while running.load(Ordering::SeqCst) {
            while let Ok(new_config) = config_update_rx.try_recv() {
                mixer.update_config(&new_config);
                debug!("Audio mixer config updated");
            }

            let mut system_disconnected = false;
            let mut mic_disconnected = false;

            // Try to receive system audio packet
            if system_packet.is_none() {
                if let Some(rx) = system_rx.as_ref() {
                    match rx.try_recv() {
                        Ok(packet) => {
                            if let Some(ref monitor) = level_monitor {
                                let samples: Vec<i16> = packet
                                    .data
                                    .chunks_exact(2)
                                    .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
                                    .collect();
                                let (left, right) = calculate_levels_stereo(&samples);
                                monitor.update_system_levels(left, right);
                            }
                            system_packet_started_at.get_or_insert_with(Instant::now);
                            system_packet = Some(packet);
                        }
                        Err(TryRecvError::Empty) => {}
                        Err(TryRecvError::Disconnected) => {
                            warn!("System audio capture channel disconnected");
                            system_disconnected = true;
                        }
                    }
                }
            }

            // Try to receive microphone audio packet
            if mic_packet.is_none() {
                if let Some(rx) = mic_rx.as_ref() {
                    match rx.try_recv() {
                        Ok(packet) => {
                            if let Some(ref monitor) = level_monitor {
                                let samples: Vec<i16> = packet
                                    .data
                                    .chunks_exact(2)
                                    .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
                                    .collect();
                                let (left, right) = calculate_levels_stereo(&samples);
                                monitor.update_mic_levels(left, right);
                            }
                            mic_packet_started_at.get_or_insert_with(Instant::now);
                            mic_packet = Some(packet);
                        }
                        Err(TryRecvError::Empty) => {}
                        Err(TryRecvError::Disconnected) => {
                            warn!("Microphone capture channel disconnected");
                            mic_disconnected = true;
                        }
                    }
                }
            }

            // Decay peak levels periodically
            if let Some(ref monitor) = level_monitor {
                if last_peak_decay.elapsed() >= Duration::from_millis(50) {
                    monitor.decay_peak_levels();
                    last_peak_decay = Instant::now();
                }
            }

            let system_wait_ready = system_packet.is_some()
                && (mic_packet.is_some()
                    || system_disconnected
                    || system_packet_started_at
                        .map(|started| started.elapsed() >= PACKET_PAIR_WAIT)
                        .unwrap_or(false));
            let mic_wait_ready = mic_packet.is_some()
                && (system_packet.is_some()
                    || mic_disconnected
                    || mic_packet_started_at
                        .map(|started| started.elapsed() >= PACKET_PAIR_WAIT)
                        .unwrap_or(false));

            // Mix audio packets if we have both, or if one stream has waited long enough.
            if system_wait_ready || mic_wait_ready {
                let had_system_packet = system_packet.is_some();
                let had_mic_packet = mic_packet.is_some();
                let mixed_packets = mixer.mix_packets(system_packet.take(), mic_packet.take());
                if had_system_packet {
                    system_packet_started_at = None;
                }
                if had_mic_packet {
                    mic_packet_started_at = None;
                }

                for mixed_packet in mixed_packets {
                    if packet_tx.send(mixed_packet).is_err() {
                        warn!("Audio manager output channel disconnected while forwarding mixed audio");
                        break;
                    }
                    forwarded_total = forwarded_total.saturating_add(1);

                    if forwarded_total == 1 || forwarded_total % 500 == 0 {
                        debug!(
                            "Audio forward: {} total packets mixed and forwarded",
                            forwarded_total
                        );
                    }
                }
            } else {
                // No packets available, sleep briefly
                thread::sleep(Duration::from_millis(1));
            }

            if system_disconnected {
                *system_rx = None;
            }
            if mic_disconnected {
                *mic_rx = None;
            }

            if system_rx.is_none() && mic_rx.is_none() {
                warn!("All audio capture channels disconnected, stopping audio forward loop");
                break;
            }
        }

        debug!(
            "Audio forward loop ended: {} total packets forwarded",
            forwarded_total
        );
    }
}

impl Drop for WasapiAudioManager {
    fn drop(&mut self) {
        self.stop();
    }
}
