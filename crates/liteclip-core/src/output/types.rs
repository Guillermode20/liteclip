#[cfg(feature = "ffmpeg")]
use super::functions::{h264_nal_type, hevc_nal_type};
use super::mp4::FfmpegMuxer;
use crate::encode::{EncodedPacket, StreamType};
use anyhow::{bail, Result};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

pub struct Muxer;

impl Muxer {
    #[cfg(feature = "ffmpeg")]
    fn detect_video_codec(video_packets: &[&EncodedPacket], fallback: &str) -> String {
        let mut saw_h264_parameter_sets = false;
        let mut saw_hevc_parameter_sets = false;

        for packet in video_packets {
            let data = packet.data.as_ref();

            match h264_nal_type(data) {
                Some(7 | 8) => saw_h264_parameter_sets = true,
                Some(1 | 5) if saw_hevc_parameter_sets => {}
                Some(1 | 5) => return "h264".to_string(),
                _ => {}
            }

            if matches!(hevc_nal_type(data), Some(32..=34)) {
                saw_hevc_parameter_sets = true;
            }

            if saw_h264_parameter_sets {
                return "h264".to_string();
            }

            if saw_hevc_parameter_sets {
                return "hevc".to_string();
            }
        }

        fallback.to_string()
    }

    #[cfg(feature = "ffmpeg")]
    pub fn mux_clip(
        output_path: &Path,
        config: &MuxerConfig,
        packets: &[EncodedPacket],
    ) -> Result<PathBuf> {
        crate::output::saver::log_save_memory("Muxer::mux_clip_entry", None, Some(packets));

        // Single-pass partition: separate video and audio by stream type in one
        // iteration instead of two filter+collect passes over the full packet list.
        let mut raw_video_packets = Vec::with_capacity(packets.len() / 2);
        let mut audio_packets = Vec::with_capacity(packets.len() / 4);
        for packet in packets {
            match packet.stream {
                StreamType::Video => raw_video_packets.push(packet),
                StreamType::SystemAudio | StreamType::Microphone => audio_packets.push(packet),
            }
        }
        if raw_video_packets.is_empty() {
            bail!("No video packets available for MP4 generation");
        }

        raw_video_packets.sort_by_key(|packet| packet.pts);
        audio_packets.sort_by_key(|packet| packet.pts);

        // Check if any standalone parameter set packets need merging.
        // If none, we can avoid the deep-copy normalization entirely.
        let needs_normalization = raw_video_packets.iter().any(|p| is_parameter_set_packet(p));

        // video_refs holds the final &EncodedPacket slices we pass to the muxer.
        // normalized_storage owns the merged packets only when normalization was needed.
        let normalized_storage: Vec<EncodedPacket>;
        let video_refs: Vec<&EncodedPacket>;

        if needs_normalization {
            normalized_storage = normalize_video_packets_for_mp4(&raw_video_packets);
            video_refs = normalized_storage.iter().collect();
            // raw_video_packets Vec is no longer needed — release it
            drop(raw_video_packets);
        } else {
            // No normalization needed — use the original reference Vec directly.
            // Avoids allocating + cloning every packet into a new owned Vec.
            video_refs = raw_video_packets;
        }

        if video_refs.is_empty() {
            bail!("No muxable video packets available for MP4 generation");
        }

        let detected_video_codec = Self::detect_video_codec(&video_refs, &config.video_codec);
        if detected_video_codec != config.video_codec {
            warn!(
                "Muxer video codec override: configured={}, detected={} from buffered packets",
                config.video_codec, detected_video_codec
            );
        }

        info!("Writing MP4 to {:?}", output_path);

        let mut muxer = FfmpegMuxer::new(
            output_path,
            &detected_video_codec,
            config.width,
            config.height,
            config.fps,
            config,
        )?;

        let (video_count, audio_count) = muxer.write_packets(&video_refs, &audio_packets)?;
        drop(muxer);

        info!(
            "MP4 finalized natively: {:?} ({} video packets, {} audio packets)",
            output_path, video_count, audio_count
        );
        Ok(output_path.to_path_buf())
    }
}

/// Merges standalone parameter-set packets (SPS/PPS/VPS) into their following
/// video frame packets so the MP4 muxer sees complete samples.
///
/// Only creates new allocations when actual merging is needed.
#[cfg(feature = "ffmpeg")]
fn normalize_video_packets_for_mp4(video_packets: &[&EncodedPacket]) -> Vec<EncodedPacket> {
    let mut normalized = Vec::with_capacity(video_packets.len());
    let mut pending_param_sets: Vec<&EncodedPacket> = Vec::new();
    let mut merged_prefix_groups = 0usize;
    let mut merged_param_set_packets = 0usize;
    let mut dropped_param_set_packets = 0usize;

    for packet in video_packets {
        if is_parameter_set_packet(packet) {
            pending_param_sets.push(*packet);
            continue;
        }

        if pending_param_sets.is_empty() {
            // No merging needed — cheap Bytes refcount clone only.
            normalized.push((*packet).clone());
            continue;
        }

        let same_timestamp = pending_param_sets
            .iter()
            .all(|param| param.pts == packet.pts && param.dts == packet.dts);

        if same_timestamp {
            let merged_len = pending_param_sets
                .iter()
                .map(|param| param.data.len())
                .sum::<usize>()
                .saturating_add(packet.data.len());
            let mut merged_data = Vec::with_capacity(merged_len);
            for param in &pending_param_sets {
                merged_data.extend_from_slice(param.data.as_ref());
            }
            merged_data.extend_from_slice(packet.data.as_ref());

            let mut merged_packet = (*packet).clone();
            merged_packet.data = merged_data.into();
            normalized.push(merged_packet);

            merged_prefix_groups += 1;
            merged_param_set_packets += pending_param_sets.len();
            pending_param_sets.clear();
        } else {
            dropped_param_set_packets += pending_param_sets.len();
            warn!(
                "Dropping {} standalone parameter-set packets before video packet at pts={} because timestamps differ",
                pending_param_sets.len(),
                packet.pts
            );
            pending_param_sets.clear();
            normalized.push((*packet).clone());
        }
    }

    if !pending_param_sets.is_empty() {
        dropped_param_set_packets += pending_param_sets.len();
        warn!(
            "Dropping {} trailing standalone parameter-set packets with no following video frame",
            pending_param_sets.len()
        );
    }

    if merged_param_set_packets > 0 {
        info!(
            "Merged {} standalone parameter-set packets into {} MP4 video samples",
            merged_param_set_packets, merged_prefix_groups
        );
    }

    if dropped_param_set_packets > 0 {
        warn!(
            "Dropped {} standalone parameter-set packets from MP4 timed samples",
            dropped_param_set_packets
        );
    }

    normalized
}

/// Returns `true` if the packet contains **only** parameter-set NAL units
/// (SPS/PPS for H.264 or VPS/SPS/PPS for HEVC) and **no** coded slice / frame data.
///
/// Some encoders (e.g. libx265) may bundle VPS+SPS+PPS+IDR into a single AVPacket.
/// This function correctly identifies those as **not** standalone parameter sets
/// by scanning all NAL units in the packet: if any VCL (coded slice) NAL appears,
/// the packet is a complete frame and must not be split off.
#[cfg(feature = "ffmpeg")]
fn is_parameter_set_packet(packet: &EncodedPacket) -> bool {
    if !matches!(packet.stream, StreamType::Video) {
        return false;
    }
    let data = packet.data.as_ref();
    if data.is_empty() {
        return false;
    }

    let mut offset = 0usize;
    let mut has_param = false;

    while offset < data.len() {
        // Locate next start code (0x00 0x00 0x01 or 0x00 0x00 0x00 0x01).
        let sc_len =
            if offset + 4 <= data.len() && data[offset..offset + 4] == [0x00, 0x00, 0x00, 0x01] {
                4
            } else if offset + 3 <= data.len() && data[offset..offset + 3] == [0x00, 0x00, 0x01] {
                3
            } else {
                offset += 1;
                continue;
            };

        let hdr = offset + sc_len;
        if hdr >= data.len() {
            break;
        }

        // Determine NAL type for HEVC (2-byte header) and H.264 (1-byte header).
        // HEVC: type in bits[7:1] of first byte, i.e. (byte >> 1) & 0x3f
        let hevc_type = (data[hdr] >> 1) & 0x3f;
        // H.264: type in bits[4:0] of first byte, i.e.  byte & 0x1f
        let h264_type = data[hdr] & 0x1f;

        // HEVC VCL (coded slice) types:
        //   0  = TRAIL_N (non-IDR)
        //   1  = TRAIL_R
        //   16 = IDR_W_RADL
        //   17 = IDR_N_LP
        //   18 = CRA_NUT
        //   19 = IDR_W_RADL  (duplicate in spec, but inclusive)
        //   20 = IDR_N_LP
        //   21 = CRA_NUT
        // etc.  Types 0-9 are VCL in HEVC, but in practice 16-21 cover IDR/CRA.
        let is_hevc_vcl = matches!(hevc_type, 0..=9 | 16..=21);
        // H.264 VCL types: 1 (non-IDR), 2-4 (A/B/C), 5 (IDR).
        let is_h264_vcl = matches!(h264_type, 1..=5);

        if is_hevc_vcl || is_h264_vcl {
            // Found frame data — this is not a standalone parameter-set packet.
            return false;
        }

        if matches!(hevc_type, 32..=34) || matches!(h264_type, 7 | 8) {
            has_param = true;
        }

        // Advance past this NAL unit to the next start code or end of data.
        let mut nal_end = hdr + 1;
        while nal_end < data.len() {
            let next_sc3 =
                nal_end + 3 <= data.len() && data[nal_end..nal_end + 3] == [0x00, 0x00, 0x01];
            let next_sc4 =
                nal_end + 4 <= data.len() && data[nal_end..nal_end + 4] == [0x00, 0x00, 0x00, 0x01];
            if next_sc3 || next_sc4 {
                break;
            }
            nal_end += 1;
        }
        offset = nal_end;
    }

    // True only if we saw at least one parameter set and zero VCL NALs.
    has_param
}

/// Configuration for the MP4 muxer.
///
/// Controls output file properties including resolution, codec, and audio settings.
#[derive(Debug, Clone)]
pub struct MuxerConfig {
    /// Video width in pixels.
    pub width: u32,
    /// Video height in pixels.
    pub height: u32,
    /// Video codec (e.g., "h264", "hevc").
    pub video_codec: String,
    /// Frames per second.
    pub fps: f64,
    /// Output file path.
    pub output_path: PathBuf,
    /// Whether to enable faststart for web streaming.
    pub faststart: bool,
    /// Whether to expect audio streams.
    pub expect_audio: bool,
}

impl MuxerConfig {
    /// Creates a new muxer configuration.
    pub fn new(width: u32, height: u32, fps: f64, output_path: impl AsRef<Path>) -> Self {
        Self {
            width,
            height,
            video_codec: "h264".to_string(),
            fps,
            output_path: output_path.as_ref().to_path_buf(),
            faststart: true,
            expect_audio: false,
        }
    }

    /// Sets the video codec.
    pub fn with_video_codec(mut self, codec: impl Into<String>) -> Self {
        self.video_codec = codec.into();
        self
    }

    /// Sets the faststart option.
    pub fn with_faststart(mut self, faststart: bool) -> Self {
        self.faststart = faststart;
        self
    }

    /// Sets whether audio is expected.
    pub fn with_expect_audio(mut self, expect_audio: bool) -> Self {
        self.expect_audio = expect_audio;
        self
    }
}
