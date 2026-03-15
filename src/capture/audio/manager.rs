//! WASAPI Audio Manager
//!
//! Coordinates system and microphone audio capture with mixing.

use anyhow::Result;
use bytes::BytesMut;
use crossbeam::channel::{Receiver, Sender, TryRecvError};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tracing::{debug, error, warn};

use crate::capture::audio::mic::WasapiMicConfig;
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
}

impl WasapiAudioManager {
    /// Create a new WASAPI audio manager
    pub fn new() -> Result<Self> {
        let (packet_tx, packet_rx) = crossbeam::channel::bounded(64);

        Ok(Self {
            system_capture: None,
            mic_capture: None,
            running: Arc::new(AtomicBool::new(false)),
            packet_tx,
            packet_rx,
            forward_thread: None,
        })
    }

    /// Start audio capture based on configuration
    pub fn start(&mut self, config: &AudioConfig) -> Result<()> {
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        debug!("Starting WASAPI audio manager...");

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
                buffer_duration: std::time::Duration::from_millis(100),
                device_id: if config.mic_device == "default" {
                    None
                } else {
                    Some(config.mic_device.clone())
                },
                noise_reduction: config.mic_noise_reduction,
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
        let mut mic_rx = self.mic_capture.as_ref().map(|capture| capture.packet_rx());
        let system_gain = (config.system_volume as f32 / 100.0).clamp(0.0, 2.0);
        let mic_gain = (config.mic_volume as f32 / 100.0).clamp(0.0, 2.0);

        self.running.store(true, Ordering::SeqCst);

        let running = Arc::clone(&self.running);
        let packet_tx = self.packet_tx.clone();
        self.forward_thread = Some(thread::spawn(move || {
            Self::forward_loop(
                running,
                packet_tx,
                &mut system_rx,
                &mut mic_rx,
                system_gain,
                mic_gain,
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

    fn forward_loop(
        running: Arc<AtomicBool>,
        packet_tx: Sender<EncodedPacket>,
        system_rx: &mut Option<Receiver<EncodedPacket>>,
        mic_rx: &mut Option<Receiver<EncodedPacket>>,
        system_gain: f32,
        mic_gain: f32,
    ) {
        let mut forwarded_total: u64 = 0;
        let mut forwarded_system: u64 = 0;
        let mut forwarded_mic: u64 = 0;
        let mut system_volume_buffer = BytesMut::new();
        let mut mic_volume_buffer = BytesMut::new();

        while running.load(Ordering::SeqCst) {
            let mut forwarded_this_tick = false;
            let mut system_disconnected = false;
            let mut mic_disconnected = false;

            if let Some(rx) = system_rx.as_ref() {
                match rx.try_recv() {
                    Ok(packet) => {
                        let packet =
                            apply_volume_to_packet(packet, system_gain, &mut system_volume_buffer);
                        if packet_tx.send(packet).is_err() {
                            warn!("Audio manager output channel disconnected while forwarding system audio");
                            break;
                        }
                        forwarded_total = forwarded_total.saturating_add(1);
                        forwarded_system = forwarded_system.saturating_add(1);
                        forwarded_this_tick = true;
                    }
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => {
                        warn!("System audio capture channel disconnected");
                        system_disconnected = true;
                    }
                }
            }

            if let Some(rx) = mic_rx.as_ref() {
                match rx.try_recv() {
                    Ok(packet) => {
                        let packet =
                            apply_volume_to_packet(packet, mic_gain, &mut mic_volume_buffer);
                        if packet_tx.send(packet).is_err() {
                            warn!("Audio manager output channel disconnected while forwarding microphone audio");
                            break;
                        }
                        forwarded_total = forwarded_total.saturating_add(1);
                        forwarded_mic = forwarded_mic.saturating_add(1);
                        forwarded_this_tick = true;
                    }
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => {
                        warn!("Microphone capture channel disconnected");
                        mic_disconnected = true;
                    }
                }
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

            if !forwarded_this_tick {
                thread::sleep(Duration::from_millis(1));
            } else if forwarded_total == 1 || forwarded_total % 500 == 0 {
                debug!(
                    "Audio forward: {} total packets (system={}, mic={})",
                    forwarded_total, forwarded_system, forwarded_mic
                );
            }
        }

        debug!(
            "Audio forward loop ended: {} total packets forwarded (system={}, mic={})",
            forwarded_total, forwarded_system, forwarded_mic
        );
    }
}

pub(crate) fn apply_volume_to_packet(
    packet: EncodedPacket,
    gain: f32,
    buffer: &mut BytesMut,
) -> EncodedPacket {
    if (gain - 1.0).abs() < f32::EPSILON || packet.data.len() < 2 {
        return packet;
    }

    buffer.clear();
    buffer.extend_from_slice(&packet.data);

    for chunk in buffer.chunks_exact_mut(2) {
        let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
        let scaled = sample as f32 * gain;

        // Custom simple soft clip for sample within standard range.
        let clipped = if scaled >= 32767.0 {
            32767.0
        } else if scaled <= -32768.0 {
            -32768.0
        } else {
            let limit = 24000.0;
            if scaled > limit {
                limit + (scaled - limit) / (1.0 + (scaled - limit) / (32767.0 - limit))
            } else if scaled < -limit {
                -limit + (scaled + limit) / (1.0 - (scaled + limit) / (32768.0 - limit))
            } else {
                scaled
            }
        };

        let final_sample = clipped.round() as i16;
        let bytes = final_sample.to_le_bytes();
        chunk[0] = bytes[0];
        chunk[1] = bytes[1];
    }

    EncodedPacket::new(
        buffer.split().freeze(),
        packet.pts,
        packet.dts,
        packet.is_keyframe,
        packet.stream,
    )
}

impl Drop for WasapiAudioManager {
    fn drop(&mut self) {
        self.stop();
    }
}
