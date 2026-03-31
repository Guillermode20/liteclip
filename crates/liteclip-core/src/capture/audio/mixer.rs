// Audio Mixer
//
// Handles real-time audio mixing for system and microphone audio.

use bytes::BytesMut;
use std::collections::VecDeque;
use tracing::warn;

use crate::config::AudioConfig;
use crate::encode::EncodedPacket;

/// Maximum packets to buffer per stream before forcing eviction.
/// At ~20ms per packet, 32 packets is ~640ms of audio.
/// This prevents unbounded memory growth if one stream stops.
const MAX_PACKETS_PER_STREAM: usize = 32;
const SILENCE_RMS_FLOOR: f32 = 1.0e-5;
const STREAM_ACTIVITY_FLOOR: f32 = 3.0e-3;
const STREAM_LEVEL_ATTACK_COEFF: f32 = 0.35;
const STREAM_LEVEL_RELEASE_COEFF: f32 = 0.12;
const STREAM_LEVEL_IDLE_DECAY: f32 = 0.92;
const STREAM_BALANCE_TOLERANCE_DB: f32 = 3.0;
const STREAM_BALANCE_MAX_BOOST_DB: f32 = 6.0;
const STREAM_BALANCE_MAX_CUT_DB: f32 = 3.0;
const STREAM_BALANCE_CUT_RATIO: f32 = 0.5;
const NORMALIZATION_MIN_GAIN: f32 = 0.25;
const NORMALIZATION_MAX_GAIN: f32 = 3.0;
const NORMALIZER_ATTACK_COEFF: f32 = 0.38;
const NORMALIZER_RELEASE_COEFF: f32 = 0.08;
const PROGRAM_LEVEL_ATTACK_COEFF: f32 = 0.08;
const PROGRAM_LEVEL_RELEASE_COEFF: f32 = 0.02;
const LIMITER_ATTACK_COEFF: f32 = 0.85;
const LIMITER_RELEASE_COEFF: f32 = 0.02;
const PCM_SCALE: f32 = 32768.0;

/// Audio packet with timestamp for queue storage.
#[derive(Debug)]
struct TimestampedPacket {
    pts: i64,
    packet: EncodedPacket,
}

/// Audio mixer for combining system and microphone audio with timestamp-based synchronization.
/// Uses VecDeque for O(1) push/pop operations instead of BTreeMap's O(log n).
pub struct AudioMixer {
    config: AudioConfig,
    system_packets: VecDeque<TimestampedPacket>,
    mic_packets: VecDeque<TimestampedPacket>,
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
    /// Reusable buffer for mixed floating-point samples in [-1, 1] domain before quantization
    mixed_float_buf: Vec<f32>,
    /// Reusable buffer for mixed samples
    mixed_samples_buf: Vec<i16>,
    /// Count of evicted packets for telemetry
    evicted_packets: u64,
    /// Pending outputs from timeout processing (later packets that need separate processing)
    extra_outputs: Vec<EncodedPacket>,
    /// Smoothed RMS tracker for system stream (linear scale)
    system_rms_ema: f32,
    /// Smoothed RMS tracker for mic stream (linear scale)
    mic_rms_ema: f32,
    /// Smoothed RMS tracker for overall program loudness (linear scale)
    program_rms_ema: f32,
    /// Smoothed global normalization gain
    normalization_gain: f32,
    /// Smoothed limiter gain (linked stereo limiter)
    limiter_gain: f32,
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
            system_packets: VecDeque::with_capacity(MAX_PACKETS_PER_STREAM),
            mic_packets: VecDeque::with_capacity(MAX_PACKETS_PER_STREAM),
            output_buffer: BytesMut::with_capacity(4096),
            sync_threshold,
            timeout,
            last_processed_pts: -1,
            system_decode_buf: Vec::with_capacity(4096 / 2), // 2 bytes per sample
            mic_decode_buf: Vec::with_capacity(4096 / 2),
            mixed_float_buf: Vec::with_capacity(4096 / 2),
            mixed_samples_buf: Vec::with_capacity(4096 / 2),
            evicted_packets: 0,
            extra_outputs: Vec::new(),
            system_rms_ema: SILENCE_RMS_FLOOR,
            mic_rms_ema: SILENCE_RMS_FLOOR,
            program_rms_ema: SILENCE_RMS_FLOOR,
            normalization_gain: 1.0,
            limiter_gain: 1.0,
        }
    }

    /// Update mixer configuration
    pub fn update_config(&mut self, config: &AudioConfig) {
        self.config = config.clone();
    }

    pub fn pending_packet_counts(&self) -> (usize, usize) {
        (self.system_packets.len(), self.mic_packets.len())
    }

    fn update_stream_level(current: &mut f32, measured: f32) {
        let coeff = if measured > *current {
            STREAM_LEVEL_ATTACK_COEFF
        } else {
            STREAM_LEVEL_RELEASE_COEFF
        };
        *current += (measured - *current) * coeff;
        *current = (*current).max(SILENCE_RMS_FLOOR);
    }

    fn update_smoothed_stream_levels(
        &mut self,
        has_system: bool,
        system_rms: f32,
        has_mic: bool,
        mic_rms: f32,
    ) {
        if has_system {
            Self::update_stream_level(&mut self.system_rms_ema, system_rms);
        } else {
            self.system_rms_ema =
                (self.system_rms_ema * STREAM_LEVEL_IDLE_DECAY).max(SILENCE_RMS_FLOOR);
        }

        if has_mic {
            Self::update_stream_level(&mut self.mic_rms_ema, mic_rms);
        } else {
            self.mic_rms_ema = (self.mic_rms_ema * STREAM_LEVEL_IDLE_DECAY).max(SILENCE_RMS_FLOOR);
        }
    }

    fn update_program_level(current: &mut f32, measured: f32) {
        let coeff = if measured > *current {
            PROGRAM_LEVEL_ATTACK_COEFF
        } else {
            PROGRAM_LEVEL_RELEASE_COEFF
        };
        *current += (measured - *current) * coeff;
        *current = (*current).max(SILENCE_RMS_FLOOR);
    }

    fn source_balance_gains(&self, has_system: bool, has_mic: bool) -> (f32, f32) {
        if !self.config.normalization_enabled || !has_system || !has_mic {
            return (1.0, 1.0);
        }

        if self.system_rms_ema < STREAM_ACTIVITY_FLOOR || self.mic_rms_ema < STREAM_ACTIVITY_FLOOR {
            return (1.0, 1.0);
        }

        let level_diff_db = linear_to_db(self.system_rms_ema) - linear_to_db(self.mic_rms_ema);
        let exceed_db = (level_diff_db.abs() - STREAM_BALANCE_TOLERANCE_DB)
            .clamp(0.0, STREAM_BALANCE_MAX_BOOST_DB);

        if exceed_db <= 0.0 {
            return (1.0, 1.0);
        }

        if level_diff_db > 0.0 {
            // System is louder: boost mic and gently trim system.
            let mic_boost = db_to_linear(exceed_db.min(STREAM_BALANCE_MAX_BOOST_DB));
            let system_cut = db_to_linear(
                -(exceed_db * STREAM_BALANCE_CUT_RATIO).min(STREAM_BALANCE_MAX_CUT_DB),
            );
            (system_cut, mic_boost)
        } else {
            // Mic is louder: boost system and gently trim mic.
            let system_boost = db_to_linear(exceed_db.min(STREAM_BALANCE_MAX_BOOST_DB));
            let mic_cut = db_to_linear(
                -(exceed_db * STREAM_BALANCE_CUT_RATIO).min(STREAM_BALANCE_MAX_CUT_DB),
            );
            (system_boost, mic_cut)
        }
    }

    fn update_normalization_gain(&mut self, mix_rms: f32, master_gain: f32) -> f32 {
        if !self.config.normalization_enabled {
            self.normalization_gain = 1.0;
            return 1.0;
        }

        if mix_rms <= SILENCE_RMS_FLOOR {
            self.normalization_gain += (1.0 - self.normalization_gain) * NORMALIZER_RELEASE_COEFF;
            self.normalization_gain = self
                .normalization_gain
                .clamp(NORMALIZATION_MIN_GAIN, NORMALIZATION_MAX_GAIN);
            return self.normalization_gain;
        }

        Self::update_program_level(&mut self.program_rms_ema, mix_rms);

        let target_level = db_to_linear(self.config.target_lufs as f32).max(SILENCE_RMS_FLOOR);
        // Adjust target to account for master_gain so final output hits the configured LUFS
        let adjusted_target = (target_level / master_gain.max(SILENCE_RMS_FLOOR))
            .max(SILENCE_RMS_FLOOR)
            .min(1.0);
        let desired_gain = (adjusted_target / self.program_rms_ema)
            .clamp(NORMALIZATION_MIN_GAIN, NORMALIZATION_MAX_GAIN);
        let coeff = if desired_gain < self.normalization_gain {
            NORMALIZER_ATTACK_COEFF
        } else {
            NORMALIZER_RELEASE_COEFF
        };
        self.normalization_gain += (desired_gain - self.normalization_gain) * coeff;
        self.normalization_gain = self
            .normalization_gain
            .clamp(NORMALIZATION_MIN_GAIN, NORMALIZATION_MAX_GAIN);
        self.normalization_gain
    }

    fn apply_limiter_frame(&mut self, left: f32, right: f32, ceiling_linear: f32) -> (f32, f32) {
        let frame_peak = left.abs().max(right.abs());
        let target_gain = if frame_peak > ceiling_linear && frame_peak > SILENCE_RMS_FLOOR {
            ceiling_linear / frame_peak
        } else {
            1.0
        };

        let coeff = if target_gain < self.limiter_gain {
            LIMITER_ATTACK_COEFF
        } else {
            LIMITER_RELEASE_COEFF
        };
        self.limiter_gain += (target_gain - self.limiter_gain) * coeff;
        self.limiter_gain = self.limiter_gain.clamp(0.05, 1.0);
        let applied_gain = self.limiter_gain.min(target_gain);

        (
            (left * applied_gain).clamp(-1.0, 1.0),
            (right * applied_gain).clamp(-1.0, 1.0),
        )
    }

    /// Insert a packet into a queue, maintaining sort order by PTS.
    /// Evicts oldest packets if the queue exceeds MAX_PACKETS_PER_STREAM.
    fn insert_sorted(
        queue: &mut VecDeque<TimestampedPacket>,
        packet: EncodedPacket,
        evicted_count: &mut u64,
    ) {
        let pts = packet.pts;
        let entry = TimestampedPacket { pts, packet };

        // Find insertion position with binary search over the contiguous slice.
        // Insert remains O(n) for VecDeque, but lookup is O(log n).
        let pos = {
            let slice = queue.make_contiguous();
            match slice.binary_search_by_key(&pts, |p| p.pts) {
                Ok(index) => index,
                Err(index) => index,
            }
        };
        queue.insert(pos, entry);

        // Evict oldest packets if we exceed the limit
        while queue.len() > MAX_PACKETS_PER_STREAM {
            queue.pop_front();
            *evicted_count += 1;
            if *evicted_count % 100 == 1 {
                warn!(
                    "Audio mixer evicted {} packets total (queue limit {})",
                    evicted_count, MAX_PACKETS_PER_STREAM
                );
            }
        }
    }

    /// Mix audio packets from system and microphone with timestamp-based synchronization
    pub fn mix_packets(
        &mut self,
        system_packet: Option<EncodedPacket>,
        mic_packet: Option<EncodedPacket>,
    ) -> Vec<EncodedPacket> {
        let mut output = Vec::new();

        // Add received packets to their respective buffers (sorted by PTS)
        if let Some(packet) = system_packet {
            Self::insert_sorted(&mut self.system_packets, packet, &mut self.evicted_packets);
        }
        if let Some(packet) = mic_packet {
            Self::insert_sorted(&mut self.mic_packets, packet, &mut self.evicted_packets);
        }

        // Drain any pending outputs from previous timeout processing
        let pending: Vec<_> = self.extra_outputs.drain(..).collect();
        for pending_packet in pending {
            let pts = pending_packet.pts;
            if let Some(mixed) = self.process_matching_packets(None, Some(&pending_packet), pts) {
                output.push(mixed);
            }
            self.last_processed_pts = pts;
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
            let system_ts = self.system_packets.front()?.pts;
            let mic_ts = self.mic_packets.front()?.pts;

            let diff = (system_ts - mic_ts).abs();

            if diff <= self.sync_threshold {
                // Packets are in sync, remove and return
                let system_packet = self.system_packets.pop_front().map(|p| p.packet);
                let mic_packet = self.mic_packets.pop_front().map(|p| p.packet);
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
                        let system_packet = self.system_packets.pop_front().map(|p| p.packet);
                        let mic_packet = self.mic_packets.pop_front().map(|p| p.packet);
                        if let Some(pkt) = mic_packet {
                            self.extra_outputs.push(pkt);
                        }
                        return Some((system_packet, None, system_ts));
                    } else {
                        let mic_packet = self.mic_packets.pop_front().map(|p| p.packet);
                        let system_packet = self.system_packets.pop_front().map(|p| p.packet);
                        if let Some(pkt) = system_packet {
                            self.extra_outputs.push(pkt);
                        }
                        return Some((None, mic_packet, mic_ts));
                    }
                } else {
                    // Packets are not close enough for sync but not yet timed out
                    // We MUST return the earlier packet if it's falling behind
                    // otherwise we stall the whole pipeline.
                    if system_ts < mic_ts {
                        let system_packet = self.system_packets.pop_front().map(|p| p.packet);
                        return Some((system_packet, None, system_ts));
                    } else {
                        let mic_packet = self.mic_packets.pop_front().map(|p| p.packet);
                        return Some((None, mic_packet, mic_ts));
                    }
                }
            }
        } else if !self.system_packets.is_empty() {
            // If only system has packets, return earliest available
            let ts = self.system_packets.front()?.pts;
            let system_packet = self.system_packets.pop_front().map(|p| p.packet);
            return Some((system_packet, None, ts));
        } else if !self.mic_packets.is_empty() {
            // If only mic has packets, return earliest available
            let ts = self.mic_packets.front()?.pts;
            let mic_packet = self.mic_packets.pop_front().map(|p| p.packet);
            return Some((None, mic_packet, ts));
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

        let has_system = system_packet.is_some();
        let has_mic = mic_packet.is_some();
        // Compute user gains first so EMA reflects post-user-gain levels for balance decisions
        let system_user_gain = (self.config.system_volume as f32 / 100.0).clamp(0.0, 2.0);
        let mic_user_gain = (self.config.mic_volume as f32 / 100.0).clamp(0.0, 4.0);
        let system_rms = calculate_rms_i16(&self.system_decode_buf) * system_user_gain;
        let mic_rms = calculate_rms_i16(&self.mic_decode_buf) * mic_user_gain;
        self.update_smoothed_stream_levels(has_system, system_rms, has_mic, mic_rms);

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

        // Calculate user gains and adaptive per-source balancing gains.
        // User volume sliders remain trim controls on top of the balancing logic.
        let (system_balance_gain, mic_balance_gain) =
            self.source_balance_gains(has_system, has_mic);
        let system_gain = system_user_gain * system_balance_gain;
        let mic_gain = mic_user_gain * mic_balance_gain;
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

        // Reuse floating mix buffer
        self.mixed_float_buf.clear();
        self.mixed_float_buf.reserve(max_samples);

        // Track unbalanced mix RMS for normalization (before panning attenuates one channel)
        let mut unbalanced_sum_sq = 0.0f64;

        // Mix and process audio in normalized float domain
        for i in 0..max_samples {
            let system_sample = self.system_decode_buf[i];
            let mic_sample = self.mic_decode_buf[i];

            // Apply per-stream gains
            let system_scaled = (system_sample as f32 / PCM_SCALE) * system_gain;
            let mic_scaled = (mic_sample as f32 / PCM_SCALE) * mic_gain;

            // Mix samples
            let mixed = system_scaled + mic_scaled;

            // Track unbalanced RMS for normalization
            unbalanced_sum_sq += f64::from(mixed) * f64::from(mixed);

            // Apply balance if stereo (even indices are left, odd are right)
            let mut balanced = mixed;
            if i % 2 == 0 {
                balanced *= left_balance;
            } else {
                balanced *= right_balance;
            }

            self.mixed_float_buf.push(balanced);
        }

        // Loudness normalization targets integrated output level in a live-friendly, smoothed way.
        // Use unbalanced RMS so panning doesn't affect the loudness target.
        let unbalanced_rms = if max_samples > 0 {
            ((unbalanced_sum_sq / max_samples as f64).sqrt() as f32).max(SILENCE_RMS_FLOOR)
        } else {
            SILENCE_RMS_FLOOR
        };
        let normalization_gain = self.update_normalization_gain(unbalanced_rms, master_gain);
        let global_gain = normalization_gain * master_gain;

        // Apply output gain and transparent limiter (linked across stereo channels).
        self.mixed_samples_buf.clear();
        self.mixed_samples_buf.reserve(max_samples);
        let limiter_enabled = self.config.true_peak_limiter_enabled;
        let limiter_ceiling = db_to_linear(self.config.true_peak_limit_dbtp as f32).clamp(0.2, 1.0);
        let mut i = 0;
        while i < max_samples {
            let left = self.mixed_float_buf[i] * global_gain;
            let right = if i + 1 < max_samples {
                self.mixed_float_buf[i + 1] * global_gain
            } else {
                left
            };

            let (left_out, right_out) = if limiter_enabled {
                self.apply_limiter_frame(left, right, limiter_ceiling)
            } else {
                self.limiter_gain += (1.0 - self.limiter_gain) * LIMITER_RELEASE_COEFF;
                (left.clamp(-1.0, 1.0), right.clamp(-1.0, 1.0))
            };

            self.mixed_samples_buf.push(float_to_i16(left_out));
            if i + 1 < max_samples {
                self.mixed_samples_buf.push(float_to_i16(right_out));
            }
            i += 2;
        }

        // Encode back to bytes
        self.output_buffer.clear();
        for sample in &self.mixed_samples_buf {
            self.output_buffer.extend_from_slice(&sample.to_le_bytes());
        }

        // Determine output stream type based on what was mixed
        let output_stream_type = if system_packet.is_some() && mic_packet.is_some() {
            // Mixed audio - use SystemAudio as the canonical type for mixed output
            crate::encode::StreamType::SystemAudio
        } else if system_packet.is_some() {
            // Only system audio
            crate::encode::StreamType::SystemAudio
        } else if mic_packet.is_some() {
            // Only microphone audio - preserve Microphone stream type
            crate::encode::StreamType::Microphone
        } else {
            // Should not happen (max_samples would be 0), but default to SystemAudio
            crate::encode::StreamType::SystemAudio
        };

        Some(EncodedPacket::new(
            self.output_buffer.split().freeze(),
            pts,
            pts,
            false,
            output_stream_type,
        ))
    }

    /// Handle packets that have timed out waiting for a matching packet
    fn handle_timeouts(&mut self) {
        let current_pts = self
            .system_packets
            .front()
            .map(|p| p.pts)
            .or_else(|| self.mic_packets.front().map(|p| p.pts));

        if let Some(current) = current_pts {
            // Check for packets that are too old compared to current earliest
            let timeout_threshold = current - self.timeout;

            // Remove system packets that have timed out
            while let Some(front) = self.system_packets.front() {
                if front.pts < timeout_threshold {
                    self.system_packets.pop_front();
                    self.evicted_packets += 1;
                } else {
                    break;
                }
            }

            // Remove mic packets that have timed out
            while let Some(front) = self.mic_packets.front() {
                if front.pts < timeout_threshold {
                    self.mic_packets.pop_front();
                    self.evicted_packets += 1;
                } else {
                    break;
                }
            }
        }
    }
}

/// Decode an EncodedPacket to i16 samples into a pre-allocated buffer
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

fn calculate_rms_i16(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return SILENCE_RMS_FLOOR;
    }

    let sum_sq = samples.iter().fold(0.0f64, |acc, &sample| {
        let normalized = f64::from(sample) / f64::from(PCM_SCALE);
        acc + normalized * normalized
    });
    ((sum_sq / samples.len() as f64).sqrt() as f32).max(SILENCE_RMS_FLOOR)
}

fn db_to_linear(db: f32) -> f32 {
    10.0f32.powf(db / 20.0)
}

fn linear_to_db(linear: f32) -> f32 {
    20.0 * linear.max(SILENCE_RMS_FLOOR).log10()
}

fn float_to_i16(sample: f32) -> i16 {
    let scaled = (sample.clamp(-1.0, 1.0) * (PCM_SCALE - 1.0)).round();
    scaled as i16
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::config_mod::Config;
    use crate::encode::EncodedPacket;
    use bytes::BytesMut;

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

        // With the gap exceeding sync threshold, packets should be processed separately
        // The gap is 1099ms, which exceeds sync_threshold (100ms) but is less than timeout (300ms)
        // The earlier packet (system) will be processed first, then the mic packet
        let result1 = mixer.mix_packets(Some(system_packet), Some(mic_packet));

        // Both packets should be processed (separately, not mixed)
        // Result should contain 2 packets: system first, then mic
        assert_eq!(
            result1.len(),
            2,
            "Expected both packets to be processed separately due to gap"
        );

        // Both queues should be empty after processing
        assert!(mixer.system_packets.is_empty());
        assert!(mixer.mic_packets.is_empty());
    }

    #[test]
    fn test_late_mic_packet_after_system_is_not_dropped() {
        let config = Config::default().audio;
        let mut mixer = AudioMixer::new(&config);

        let mut system_data = BytesMut::with_capacity(4);
        system_data.extend_from_slice(&1000i16.to_le_bytes());
        system_data.extend_from_slice(&1000i16.to_le_bytes());
        let system_packet = EncodedPacket::new(
            system_data.freeze(),
            1000,
            1000,
            false,
            crate::encode::StreamType::SystemAudio,
        );

        // First call processes system packet by itself and advances last_processed_pts.
        let first = mixer.mix_packets(Some(system_packet), None);
        assert_eq!(first.len(), 1);
        assert!(matches!(
            first[0].stream,
            crate::encode::StreamType::SystemAudio
        ));

        // Mic packet arrives later but has very close PTS (normal cross-thread arrival skew).
        // It must still be emitted, not discarded as stale.
        let mut mic_data = BytesMut::with_capacity(4);
        mic_data.extend_from_slice(&2000i16.to_le_bytes());
        mic_data.extend_from_slice(&2000i16.to_le_bytes());
        let mic_packet = EncodedPacket::new(
            mic_data.freeze(),
            1001,
            1001,
            false,
            crate::encode::StreamType::Microphone,
        );

        let second = mixer.mix_packets(None, Some(mic_packet));
        assert_eq!(
            second.len(),
            1,
            "late mic packet should be forwarded instead of dropped"
        );
        assert!(
            matches!(second[0].stream, crate::encode::StreamType::Microphone),
            "late packet stream type should remain microphone when no system packet is paired"
        );
    }

    #[test]
    fn test_max_packets_limit() {
        let config = Config::default().audio;
        let mut mixer = AudioMixer::new(&config);

        // Insert more than MAX_PACKETS_PER_STREAM packets
        // mix_packets will process them, but the queue should never exceed the limit
        for i in 0..(MAX_PACKETS_PER_STREAM * 2) {
            let mut data = BytesMut::with_capacity(4);
            data.extend_from_slice(&1000i16.to_le_bytes());
            data.extend_from_slice(&2000i16.to_le_bytes());
            let packet = EncodedPacket::new(
                data.freeze(),
                i as i64 * 100000, // 10ms apart, sorted increasing
                i as i64 * 100000,
                false,
                crate::encode::StreamType::SystemAudio,
            );
            mixer.mix_packets(Some(packet), None);

            // Check limit after each insert
            assert!(
                mixer.system_packets.len() <= MAX_PACKETS_PER_STREAM,
                "Queue length {} exceeds limit {} at iteration {}",
                mixer.system_packets.len(),
                MAX_PACKETS_PER_STREAM,
                i
            );
        }

        // At least some packets should have been processed/evicted
        assert!(
            mixer.evicted_packets > 0 || mixer.pending_packet_counts().0 < MAX_PACKETS_PER_STREAM,
            "Expected some packets to be evicted or processed"
        );
    }

    #[test]
    fn test_normalization_prefers_boosting_quiet_source() {
        let mut config = Config::default().audio;
        config.normalization_enabled = true;
        let mut mixer = AudioMixer::new(&config);
        mixer.system_rms_ema = db_to_linear(-8.0);
        mixer.mic_rms_ema = db_to_linear(-28.0);

        let (system_gain, mic_gain) = mixer.source_balance_gains(true, true);
        assert!(
            mic_gain > 1.0,
            "Expected mic boost when mic is much quieter"
        );
        assert!(
            system_gain <= 1.0,
            "Expected system trim when system is much louder"
        );
    }

    #[test]
    fn test_target_lufs_maps_to_expected_linear_level() {
        let mut config = Config::default().audio;
        config.target_lufs = -16;
        let mut mixer = AudioMixer::new(&config);
        let target_linear = db_to_linear(config.target_lufs as f32);
        mixer.program_rms_ema = target_linear;
        mixer.update_normalization_gain(target_linear, 1.0);
        assert!(
            (mixer.normalization_gain - 1.0).abs() < 0.25,
            "Normalization gain should remain near unity when at target loudness"
        );
    }

    #[test]
    fn test_transient_quiet_sections_do_not_overboost() {
        let mut config = Config::default().audio;
        config.normalization_enabled = true;
        let mut mixer = AudioMixer::new(&config);
        mixer.program_rms_ema = db_to_linear(-18.0);
        mixer.normalization_gain = 1.0;

        for _ in 0..3 {
            mixer.update_normalization_gain(db_to_linear(-40.0), 1.0);
        }

        assert!(
            mixer.normalization_gain < 1.3,
            "Short quiet passages should not cause strong gain pumping"
        );
    }

    #[test]
    fn test_limiter_caps_peaks_to_configured_ceiling() {
        let mut config = Config::default().audio;
        config.true_peak_limiter_enabled = true;
        config.true_peak_limit_dbtp = -1;
        let mut mixer = AudioMixer::new(&config);
        let ceiling = db_to_linear(config.true_peak_limit_dbtp as f32);

        let mut max_peak = 0.0f32;
        for _ in 0..32 {
            let (l, r) = mixer.apply_limiter_frame(1.8, -1.7, ceiling);
            max_peak = max_peak.max(l.abs()).max(r.abs());
        }

        assert!(
            max_peak <= ceiling + 0.05,
            "Limiter output peak {} should not exceed ceiling {} by more than smoothing margin",
            max_peak,
            ceiling
        );
    }
}
