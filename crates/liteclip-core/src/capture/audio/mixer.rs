// Audio Mixer
//
// Handles real-time audio mixing for system and microphone audio.

use bytes::BytesMut;
use std::collections::BTreeMap;

use crate::config::AudioConfig;
use crate::encode::EncodedPacket;

/// Audio mixer for combining system and microphone audio with timestamp-based synchronization
pub struct AudioMixer {
    config: AudioConfig,
    system_packets: BTreeMap<i64, EncodedPacket>, // PTS -> Packet (sorted for fast access)
    mic_packets: BTreeMap<i64, EncodedPacket>,    // PTS -> Packet (sorted for fast access)
    output_buffer: BytesMut,
    /// Maximum time difference (in QPC ticks) allowed between packets to be considered matching
    sync_threshold: i64,
    /// Timeout for waiting for matching packets (in QPC ticks)
    timeout: i64,
    /// Last processed timestamp to track synchronization progress
    last_processed_pts: i64,
    /// Reusable buffers for decoding packets
    system_decode_buf: Vec<i16>,
    mic_decode_buf: Vec<i16>,
    /// Reusable buffer for mixed samples
    mixed_samples_buf: Vec<i16>,
}

impl AudioMixer {
    /// Create a new audio mixer with synchronization capabilities
    pub fn new(config: &AudioConfig) -> Self {
        // Calculate synchronization parameters (QPC ticks)
        // QPC frequency is ~10MHz (1 tick = 100ns)
        // Audio packets are 100ms duration
        // Increased tolerance to handle varying packet timings better
        const QPC_TICKS_PER_MILLISECOND: i64 = 10_000; // ~10MHz
        let sync_threshold = 100 * QPC_TICKS_PER_MILLISECOND; // 100ms tolerance
        let timeout = 300 * QPC_TICKS_PER_MILLISECOND; // 300ms timeout

        Self {
            config: config.clone(),
            system_packets: BTreeMap::new(),
            mic_packets: BTreeMap::new(),
            output_buffer: BytesMut::with_capacity(4096),
            sync_threshold,
            timeout,
            last_processed_pts: -1,
            system_decode_buf: Vec::with_capacity(4096 / 2), // 2 bytes per sample
            mic_decode_buf: Vec::with_capacity(4096 / 2),
            mixed_samples_buf: Vec::with_capacity(4096 / 2),
        }
    }

    /// Update mixer configuration
    pub fn update_config(&mut self, config: &AudioConfig) {
        self.config = config.clone();
    }

    /// Mix audio packets from system and microphone with timestamp-based synchronization
    pub fn mix_packets(
        &mut self,
        system_packet: Option<EncodedPacket>,
        mic_packet: Option<EncodedPacket>,
    ) -> Vec<EncodedPacket> {
        let mut output = Vec::new();

        // Add received packets to their respective buffers
        if let Some(packet) = system_packet {
            self.system_packets.insert(packet.pts, packet);
        }
        if let Some(packet) = mic_packet {
            self.mic_packets.insert(packet.pts, packet);
        }

        // Try to find matching packets for synchronization
        while let Some((system_packet, mic_packet, pts)) = self.find_matching_packets() {
            // Process the matching packets
            if let Some(mixed) =
                self.process_matching_packets(system_packet.as_ref(), mic_packet.as_ref(), pts)
            {
                output.push(mixed);
            }

            // Update last processed timestamp
            self.last_processed_pts = pts;
        }

        // Handle timeout for packets that are too old
        self.handle_timeouts();

        output
    }

    /// Find and remove the earliest matching packets from both streams
    fn find_matching_packets(
        &mut self,
    ) -> Option<(Option<EncodedPacket>, Option<EncodedPacket>, i64)> {
        // If we have packets from both streams
        if !self.system_packets.is_empty() && !self.mic_packets.is_empty() {
            // Get earliest system and mic packets after last processed timestamp
            let system_pts_iter = self
                .system_packets
                .keys()
                .filter(|&&ts| ts > self.last_processed_pts);
            let mic_pts_iter = self
                .mic_packets
                .keys()
                .filter(|&&ts| ts > self.last_processed_pts);

            if let Some(&system_ts) = system_pts_iter.clone().next() {
                if let Some(&mic_ts) = mic_pts_iter.clone().next() {
                    let diff = (system_ts - mic_ts).abs();

                    if diff <= self.sync_threshold {
                        // Packets are in sync, remove and return
                        let system_packet = self.system_packets.remove(&system_ts);
                        let mic_packet = self.mic_packets.remove(&mic_ts);
                        return Some((system_packet, mic_packet, system_ts.min(mic_ts)));
                    } else {
                        // Packets are not in sync - process the earlier packet and pad the other
                        let (earlier_ts, later_ts) = if system_ts < mic_ts {
                            (system_ts, mic_ts)
                        } else {
                            (mic_ts, system_ts)
                        };

                        // If the gap is too large, process the earlier packet and pad
                        if later_ts - earlier_ts > self.timeout {
                            if system_ts < mic_ts {
                                let system_packet = self.system_packets.remove(&system_ts);
                                return Some((system_packet, None, system_ts));
                            } else {
                                let mic_packet = self.mic_packets.remove(&mic_ts);
                                return Some((None, mic_packet, mic_ts));
                            }
                        } else {
                            // Packets are not close enough for sync but not yet timed out
                            // We MUST return the earlier packet if it's falling behind
                            // otherwise we stall the whole pipeline.
                            if system_ts < mic_ts {
                                let system_packet = self.system_packets.remove(&system_ts);
                                return Some((system_packet, None, system_ts));
                            } else {
                                let mic_packet = self.mic_packets.remove(&mic_ts);
                                return Some((None, mic_packet, mic_ts));
                            }
                        }
                    }
                }
            }
        } else if !self.system_packets.is_empty() {
            // If only system has packets, return earliest available
            if let Some(&ts) = self
                .system_packets
                .keys()
                .find(|&&ts| ts > self.last_processed_pts)
            {
                let system_packet = self.system_packets.remove(&ts);
                return Some((system_packet, None, ts));
            }
        } else if !self.mic_packets.is_empty() {
            // If only mic has packets, return earliest available
            if let Some(&ts) = self
                .mic_packets
                .keys()
                .find(|&&ts| ts > self.last_processed_pts)
            {
                let mic_packet = self.mic_packets.remove(&ts);
                return Some((None, mic_packet, ts));
            }
        }

        None
    }

    /// Process packets with matching timestamps
    fn process_matching_packets(
        &mut self,
        system_packet: Option<&EncodedPacket>,
        mic_packet: Option<&EncodedPacket>,
        pts: i64,
    ) -> Option<EncodedPacket> {
        // Decode packets into reusable buffers
        self.system_decode_buf.clear();
        self.mic_decode_buf.clear();

        if let Some(packet) = system_packet {
            decode_packet_into(packet, &mut self.system_decode_buf);
        }

        if let Some(packet) = mic_packet {
            decode_packet_into(packet, &mut self.mic_decode_buf);
        }

        // Determine the maximum buffer size
        let max_samples = self.system_decode_buf.len().max(self.mic_decode_buf.len());

        if max_samples == 0 {
            return None;
        }

        // Resize buffers to match
        if self.system_decode_buf.len() < max_samples {
            self.system_decode_buf.resize(max_samples, 0);
        }
        if self.mic_decode_buf.len() < max_samples {
            self.mic_decode_buf.resize(max_samples, 0);
        }

        // Calculate gains
        let system_gain = (self.config.system_volume as f32 / 100.0).clamp(0.0, 2.0);
        let mic_gain = (self.config.mic_volume as f32 / 100.0).clamp(0.0, 1.0);
        let master_gain = (self.config.master_volume as f32 / 100.0).clamp(0.0, 2.0);

        // Calculate balance (stereo only)
        let (left_balance, right_balance) = if self.config.balance < 0 {
            // Left bias
            let bias = (self.config.balance as f32 / -100.0).clamp(0.0, 1.0);
            (1.0, 1.0 - bias)
        } else {
            // Right bias
            let bias = (self.config.balance as f32 / 100.0).clamp(0.0, 1.0);
            (1.0 - bias, 1.0)
        };

        // Reuse mixed samples buffer
        self.mixed_samples_buf.clear();
        self.mixed_samples_buf.reserve(max_samples);

        // Mix and process audio
        for i in 0..max_samples {
            let system_sample = self.system_decode_buf[i];
            let mic_sample = self.mic_decode_buf[i];

            // Apply per-stream gains
            let system_scaled = (system_sample as f32) * system_gain;
            let mic_scaled = (mic_sample as f32) * mic_gain;

            // Mix samples
            let mixed = system_scaled + mic_scaled;

            // Apply balance if stereo (even indices are left, odd are right)
            let mut balanced = mixed;
            if i % 2 == 0 {
                balanced *= left_balance;
            } else {
                balanced *= right_balance;
            }

            // Apply master volume
            let final_sample = balanced * master_gain;

            // Simple hard clipping
            let clipped = final_sample.clamp(-32768.0, 32767.0);

            self.mixed_samples_buf.push(clipped.round() as i16);
        }

        // Encode back to bytes
        self.output_buffer.clear();
        for sample in &self.mixed_samples_buf {
            self.output_buffer.extend_from_slice(&sample.to_le_bytes());
        }

        Some(EncodedPacket::new(
            self.output_buffer.split().freeze(),
            pts,
            pts,
            false,
            crate::encode::StreamType::SystemAudio, // Use SystemAudio for mixed output
        ))
    }

    /// Handle packets that have timed out waiting for a matching packet
    fn handle_timeouts(&mut self) {
        let current_pts = self
            .system_packets
            .keys()
            .chain(self.mic_packets.keys())
            .cloned()
            .min();

        if let Some(current) = current_pts {
            // Check for packets that are too old compared to current earliest
            let timeout_threshold = current - self.timeout;

            // Remove system packets that have timed out
            let system_to_remove: Vec<_> = self
                .system_packets
                .keys()
                .filter(|&&ts| ts < timeout_threshold)
                .cloned()
                .collect();
            for ts in system_to_remove {
                self.system_packets.remove(&ts);
            }

            // Remove mic packets that have timed out
            let mic_to_remove: Vec<_> = self
                .mic_packets
                .keys()
                .filter(|&&ts| ts < timeout_threshold)
                .cloned()
                .collect();
            for ts in mic_to_remove {
                self.mic_packets.remove(&ts);
            }
        }
    }
}

/// Decode an EncodedPacket to j16 samples into a pre-allocated buffer
fn decode_packet_into(packet: &EncodedPacket, buffer: &mut Vec<i16>) {
    buffer.clear();
    buffer.reserve(packet.data.len() / 2);
    buffer.extend(
        packet
            .data
            .chunks_exact(2)
            .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]])),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::config_mod::Config;
    use bytes::BytesMut;
    use crate::encode::EncodedPacket;

    #[test]
    fn test_mixer_basic() {
        let config = Config::default().audio;
        let mut mixer = AudioMixer::new(&config);

        // Create test packets
        let mut system_data = BytesMut::with_capacity(4);
        system_data.extend_from_slice(&1000i16.to_le_bytes());
        system_data.extend_from_slice(&2000i16.to_le_bytes());
        let system_packet = EncodedPacket::new(
            system_data.freeze(),
            0,
            0,
            false,
            crate::encode::StreamType::SystemAudio,
        );

        let mut mic_data = BytesMut::with_capacity(4);
        mic_data.extend_from_slice(&3000i16.to_le_bytes());
        mic_data.extend_from_slice(&4000i16.to_le_bytes());
        let mic_packet = EncodedPacket::new(
            mic_data.freeze(),
            0,
            0,
            false,
            crate::encode::StreamType::Microphone,
        );

        let result = mixer.mix_packets(Some(system_packet), Some(mic_packet));
        assert!(!result.is_empty());
    }

    #[test]
    fn test_synchronization() {
        let config = Config::default().audio;
        let mut mixer = AudioMixer::new(&config);

        // Create test packets with slightly different timestamps
        let mut system_data = BytesMut::with_capacity(4);
        system_data.extend_from_slice(&1000i16.to_le_bytes());
        system_data.extend_from_slice(&2000i16.to_le_bytes());
        let system_packet = EncodedPacket::new(
            system_data.freeze(),
            1000, // 100us
            1000,
            false,
            crate::encode::StreamType::SystemAudio,
        );

        let mut mic_data = BytesMut::with_capacity(4);
        mic_data.extend_from_slice(&3000i16.to_le_bytes());
        mic_data.extend_from_slice(&4000i16.to_le_bytes());
        let mic_packet = EncodedPacket::new(
            mic_data.freeze(),
            1050, // 105us (within sync threshold)
            1050,
            false,
            crate::encode::StreamType::Microphone,
        );

        let result = mixer.mix_packets(Some(system_packet), Some(mic_packet));
        assert!(!result.is_empty());
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_sync_threshold() {
        let config = Config::default().audio;
        let mut mixer = AudioMixer::new(&config);

        // Create test packets with timestamps exceeding sync threshold
        let mut system_data = BytesMut::with_capacity(4);
        system_data.extend_from_slice(&1000i16.to_le_bytes());
        system_data.extend_from_slice(&2000i16.to_le_bytes());
        let system_packet = EncodedPacket::new(
            system_data.freeze(),
            1000,
            1000,
            false,
            crate::encode::StreamType::SystemAudio,
        );

        let mut mic_data = BytesMut::with_capacity(4);
        mic_data.extend_from_slice(&3000i16.to_le_bytes());
        mic_data.extend_from_slice(&4000i16.to_le_bytes());
        let mic_packet = EncodedPacket::new(
            mic_data.freeze(),
            1100000, // 110ms (exceeds 100ms sync threshold)
            1100000,
            false,
            crate::encode::StreamType::Microphone,
        );

        // First call should buffer both packets
        let result1 = mixer.mix_packets(Some(system_packet), Some(mic_packet));
        assert!(result1.is_empty());

        // Verify packets are buffered
        assert!(!mixer.system_packets.is_empty());
        assert!(!mixer.mic_packets.is_empty());
    }
}
