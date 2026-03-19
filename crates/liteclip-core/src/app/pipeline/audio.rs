use crate::{
    buffer::ReplayBuffer,
    capture::audio::{AudioLevelMonitor, WasapiAudioManager},
    config::Config,
};
use anyhow::{Context, Result};
use tracing::{debug, info};

pub fn start_audio_capture(
    config: &Config,
    buffer: &ReplayBuffer,
    context: &str,
    level_monitor: Option<AudioLevelMonitor>,
) -> Result<WasapiAudioManager> {
    if !config.audio.capture_system && !config.audio.capture_mic {
        return WasapiAudioManager::new();
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

    info!("Audio capture started ({})", context_label);
    Ok(audio_manager)
}
