//! Audio Mixer
//!
//! Combines system audio and microphone audio streams with volume controls.

use anyhow::Result;
use crossbeam::channel::{bounded, Receiver, Sender, TryRecvError};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

use crate::encode::{EncodedPacket, StreamType};

/// Configuration for audio mixing
#[derive(Debug, Clone)]
pub struct AudioMixerConfig {
    pub system_volume: f32, // 0.0 to 1.0
    pub mic_volume: f32,    // 0.0 to 1.0
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
}

impl Default for AudioMixerConfig {
    fn default() -> Self {
        Self {
            system_volume: 1.0, // 100% volume
            mic_volume: 1.0,    // 100% volume
            sample_rate: 48000,
            channels: 2,
            bits_per_sample: 16,
        }
    }
}

/// Audio mixer implementation
pub struct AudioMixer {
    running: Arc<AtomicBool>,
    system_rx: Option<Receiver<EncodedPacket>>,
    mic_rx: Option<Receiver<EncodedPacket>>,
    packet_tx: Sender<EncodedPacket>,
    packet_rx: Receiver<EncodedPacket>,
    mixed_samples: Arc<AtomicU64>,
}

impl AudioMixer {
    /// Create a new audio mixer instance
    pub fn new() -> Result<Self> {
        let (packet_tx, packet_rx) = bounded(64); // Buffer for mixed audio packets

        Ok(Self {
            running: Arc::new(AtomicBool::new(false)),
            system_rx: None,
            mic_rx: None,
            packet_tx,
            packet_rx,
            mixed_samples: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Set the system audio receiver
    pub fn set_system_rx(&mut self, rx: Receiver<EncodedPacket>) {
        self.system_rx = Some(rx);
    }

    /// Set the microphone audio receiver
    pub fn set_mic_rx(&mut self, rx: Receiver<EncodedPacket>) {
        self.mic_rx = Some(rx);
    }

    /// Start the audio mixer
    pub fn start(&mut self, config: AudioMixerConfig) -> Result<()> {
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        self.running.store(true, Ordering::SeqCst);

        let running = Arc::clone(&self.running);
        let system_rx = self.system_rx.take();
        let mic_rx = self.mic_rx.take();
        let packet_tx = self.packet_tx.clone();
        let mixed_samples = Arc::clone(&self.mixed_samples);

        // Spawn the mixing thread
        thread::spawn(move || {
            if let Err(e) =
                Self::mix_loop(running, system_rx, mic_rx, packet_tx, mixed_samples, config)
            {
                error!("Audio mixer error: {}", e);
            }
        });

        info!("Audio mixer started");
        Ok(())
    }

    /// Stop the audio mixer
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        info!("Audio mixer stopped");
    }

    /// Get receiver for mixed audio packets
    pub fn packet_rx(&self) -> Receiver<EncodedPacket> {
        self.packet_rx.clone()
    }

    /// Get the number of samples mixed
    pub fn samples_mixed(&self) -> u64 {
        self.mixed_samples.load(Ordering::SeqCst)
    }

    /// Main mixing loop
    fn mix_loop(
        running: Arc<AtomicBool>,
        mut system_rx: Option<Receiver<EncodedPacket>>,
        mut mic_rx: Option<Receiver<EncodedPacket>>,
        packet_tx: Sender<EncodedPacket>,
        mixed_samples: Arc<AtomicU64>,
        config: AudioMixerConfig,
    ) -> Result<()> {
        debug!("Starting audio mixing loop");

        if system_rx.is_none() && mic_rx.is_none() {
            return Err(anyhow::anyhow!("No audio receiver configured for mixer"));
        }

        let mut pending_system: Option<(EncodedPacket, Instant)> = None;
        let mut pending_mic: Option<(EncodedPacket, Instant)> = None;
        let pending_timeout = Duration::from_millis(20);
        let mut output_packets: u64 = 0;
        let mut seen_system_input = false;
        let mut seen_mic_input = false;

        // Main mixing loop
        while running.load(Ordering::SeqCst) {
            // Collect newly arrived packets
            if let Some(ref rx) = system_rx {
                match rx.try_recv() {
                    Ok(packet) => {
                        if !seen_system_input {
                            info!("Audio mixer received first system input packet");
                            seen_system_input = true;
                        }
                        pending_system = Some((packet, Instant::now()));
                    }
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => {
                        warn!("System audio receiver disconnected");
                        system_rx = None;
                    }
                }
            }

            if let Some(ref rx) = mic_rx {
                match rx.try_recv() {
                    Ok(packet) => {
                        if !seen_mic_input {
                            info!("Audio mixer received first microphone input packet");
                            seen_mic_input = true;
                        }
                        pending_mic = Some((packet, Instant::now()));
                    }
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => {
                        warn!("Microphone audio receiver disconnected");
                        mic_rx = None;
                    }
                }
            }

            if system_rx.is_none() && mic_rx.is_none() {
                warn!("All audio receivers disconnected, stopping mixer loop");
                break;
            }

            // Two-source mode: mix when both are available.
            // IMPORTANT: avoid taking one side when the other is missing,
            // otherwise single-source packets get dropped before fallback.
            if pending_system.is_some() && pending_mic.is_some() {
                let (system_packet, _) = pending_system.take().expect("checked is_some");
                let (mic_packet, _) = pending_mic.take().expect("checked is_some");
                let mixed = mix_packets(
                    system_packet,
                    mic_packet,
                    config.system_volume,
                    config.mic_volume,
                )?;
                let sample_count = (mixed.data.len() / 2) as u64;

                if packet_tx.send(mixed).is_err() {
                    warn!("Audio mixer output channel disconnected while sending mixed packet");
                    break;
                }

                output_packets = output_packets.saturating_add(1);
                if output_packets == 1 {
                    info!("Audio mixer emitted first mixed packet");
                } else if output_packets % 500 == 0 {
                    debug!("Audio mixer emitted {} packets", output_packets);
                }

                mixed_samples.fetch_add(sample_count, Ordering::SeqCst);
                continue;
            }

            // Single-source fallback (or when the counterpart is delayed too long).
            let now = Instant::now();

            if let Some((packet, ts)) = pending_system.take() {
                if mic_rx.is_none() || now.duration_since(ts) >= pending_timeout {
                    let adjusted = apply_volume_to_packet(packet, config.system_volume)?;
                    let sample_count = (adjusted.data.len() / 2) as u64;

                    if packet_tx.send(adjusted).is_err() {
                        warn!(
                            "Audio mixer output channel disconnected while sending system packet"
                        );
                        break;
                    }

                    output_packets = output_packets.saturating_add(1);
                    if output_packets == 1 {
                        info!("Audio mixer emitted first packet (system source)");
                    } else if output_packets % 500 == 0 {
                        debug!("Audio mixer emitted {} packets", output_packets);
                    }

                    mixed_samples.fetch_add(sample_count, Ordering::SeqCst);
                } else {
                    pending_system = Some((packet, ts));
                }
            }

            if let Some((packet, ts)) = pending_mic.take() {
                if system_rx.is_none() || now.duration_since(ts) >= pending_timeout {
                    let adjusted = apply_volume_to_packet(packet, config.mic_volume)?;
                    let sample_count = (adjusted.data.len() / 2) as u64;

                    if packet_tx.send(adjusted).is_err() {
                        warn!("Audio mixer output channel disconnected while sending microphone packet");
                        break;
                    }

                    output_packets = output_packets.saturating_add(1);
                    if output_packets == 1 {
                        info!("Audio mixer emitted first packet (microphone source)");
                    } else if output_packets % 500 == 0 {
                        debug!("Audio mixer emitted {} packets", output_packets);
                    }

                    mixed_samples.fetch_add(sample_count, Ordering::SeqCst);
                } else {
                    pending_mic = Some((packet, ts));
                }
            }

            thread::sleep(Duration::from_millis(1));
        }

        info!("Audio mixing loop ended");
        Ok(())
    }
}

/// Apply volume adjustment to an audio packet
fn apply_volume_to_packet(packet: EncodedPacket, volume: f32) -> Result<EncodedPacket> {
    let volume_multiplier = volume.clamp(0.0, 1.0);

    if (volume_multiplier - 1.0).abs() < f32::EPSILON {
        return Ok(packet);
    }

    if packet.data.len() < 2 {
        return Ok(packet);
    }

    let mut data = packet.data.to_vec();

    for chunk in data.chunks_exact_mut(2) {
        let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
        let scaled = (sample as f32 * volume_multiplier)
            .round()
            .clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        let bytes = scaled.to_le_bytes();
        chunk[0] = bytes[0];
        chunk[1] = bytes[1];
    }

    Ok(EncodedPacket::new(
        data,
        packet.pts,
        packet.dts,
        packet.is_keyframe,
        packet.stream,
    ))
}

fn mix_packets(
    system_packet: EncodedPacket,
    mic_packet: EncodedPacket,
    system_volume: f32,
    mic_volume: f32,
) -> Result<EncodedPacket> {
    let system_data = system_packet.data.as_ref();
    let mic_data = mic_packet.data.as_ref();

    let max_len = system_data.len().max(mic_data.len());
    if max_len < 2 {
        return Ok(EncodedPacket::new(
            Vec::<u8>::new(),
            system_packet.pts.min(mic_packet.pts),
            system_packet.dts.min(mic_packet.dts),
            false,
            StreamType::SystemAudio,
        ));
    }

    let len_aligned = max_len - (max_len % 2);
    let mut mixed = vec![0u8; len_aligned];

    let sys_gain = system_volume.clamp(0.0, 1.0);
    let mic_gain = mic_volume.clamp(0.0, 1.0);

    for i in (0..len_aligned).step_by(2) {
        let sys = if i + 1 < system_data.len() {
            i16::from_le_bytes([system_data[i], system_data[i + 1]]) as f32
        } else {
            0.0
        };

        let mic = if i + 1 < mic_data.len() {
            i16::from_le_bytes([mic_data[i], mic_data[i + 1]]) as f32
        } else {
            0.0
        };

        let sample = (sys * sys_gain + mic * mic_gain)
            .round()
            .clamp(i16::MIN as f32, i16::MAX as f32) as i16;

        let bytes = sample.to_le_bytes();
        mixed[i] = bytes[0];
        mixed[i + 1] = bytes[1];
    }

    Ok(EncodedPacket::new(
        mixed,
        system_packet.pts.min(mic_packet.pts),
        system_packet.dts.min(mic_packet.dts),
        false,
        StreamType::SystemAudio,
    ))
}

impl Drop for AudioMixer {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_mixer_config_default() {
        let config = AudioMixerConfig::default();
        assert_eq!(config.system_volume, 1.0);
        assert_eq!(config.mic_volume, 1.0);
        assert_eq!(config.sample_rate, 48000);
        assert_eq!(config.channels, 2);
        assert_eq!(config.bits_per_sample, 16);
    }
}
