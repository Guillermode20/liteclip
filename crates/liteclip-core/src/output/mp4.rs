use crate::encode::EncodedPacket;
use anyhow::{Context, Result};
use ffmpeg_next as ffmpeg;
use std::path::{Path, PathBuf};
use tracing::info;

use super::{
    functions::{AUDIO_CHANNELS, AUDIO_SAMPLE_RATE},
    MuxerConfig,
};

const AUDIO_BITRATE_BPS: usize = 192_000;
const PCM_BYTES_PER_SAMPLE: usize = 2;
const AUDIO_PACKET_JITTER_TOLERANCE_FRAMES: usize = 8;

struct EncodedAudioPacket {
    data: bytes::Bytes,
    pts: i64,
    duration: i64,
}

pub struct FfmpegMuxer {
    format_context: ffmpeg::format::context::Output,
    video_stream_index: usize,
    audio_stream_index: Option<usize>,
    audio_encoder: Option<ffmpeg::encoder::Audio>,
    output_path: PathBuf,
    video_time_base: (i32, i32),
    video_frame_rate: i32,
    audio_time_base: (i32, i32),
    expect_audio: bool,
    faststart: bool,
}

impl FfmpegMuxer {
    pub fn new(
        output_path: &Path,
        video_codec: &str,
        width: u32,
        height: u32,
        fps: f64,
        config: &MuxerConfig,
    ) -> Result<Self> {
        crate::output::saver::log_save_memory("FfmpegMuxer::new_entry", None, None);
        let mut format_context = ffmpeg::format::output(&output_path)
            .context("Failed to create output format context")?;

        let rounded_fps = fps.round().clamp(1.0, i32::MAX as f64) as i32;
        let video_time_base = (1, 90_000);
        let audio_time_base = (1, AUDIO_SAMPLE_RATE as i32);

        let global_header = format_context
            .format()
            .flags()
            .contains(ffmpeg::format::flag::Flags::GLOBAL_HEADER);

        let video_stream_index = {
            let codec_id = match video_codec {
                "hevc" => ffmpeg::codec::Id::HEVC,
                "av1" => ffmpeg::codec::Id::AV1,
                _ => ffmpeg::codec::Id::H264,
            };

            let codec =
                ffmpeg::encoder::find(codec_id).context("Failed to find video codec for muxer")?;

            let mut stream = format_context.add_stream(codec)?;
            let stream_index = stream.index();

            stream.set_time_base(video_time_base);
            stream.set_avg_frame_rate((rounded_fps, 1));

            let mut video = ffmpeg::codec::context::Context::new_with_codec(codec)
                .encoder()
                .video()?;
            video.set_width(width);
            video.set_height(height);
            video.set_format(ffmpeg::format::Pixel::YUV420P);
            video.set_time_base(video_time_base);
            video.set_frame_rate(Some((rounded_fps, 1)));
            stream.set_parameters(&video);

            stream_index
        };

        let mut audio_stream_index = None;
        let mut audio_encoder = None;
        if config.expect_audio {
            let codec = ffmpeg::encoder::find(ffmpeg::codec::Id::AAC)
                .context("Failed to find AAC encoder for muxer")?
                .audio()
                .context("Failed to use AAC encoder for audio stream")?;
            let sample_format = codec
                .formats()
                .and_then(|mut formats| formats.next())
                .context("AAC encoder did not report a supported sample format")?;

            let mut stream = format_context.add_stream(codec)?;
            let stream_index = stream.index();

            let mut audio = ffmpeg::codec::context::Context::new_with_codec(*codec)
                .encoder()
                .audio()?;
            audio.set_rate(AUDIO_SAMPLE_RATE as i32);
            audio.set_channel_layout(ffmpeg::channel_layout::ChannelLayout::STEREO);
            audio.set_format(sample_format);
            audio.set_bit_rate(AUDIO_BITRATE_BPS);
            audio.set_max_bit_rate(AUDIO_BITRATE_BPS);
            audio.set_time_base(audio_time_base);
            if global_header {
                audio.set_flags(ffmpeg::codec::flag::Flags::GLOBAL_HEADER);
            }

            let mut aac_opts = ffmpeg::Dictionary::new();
            aac_opts.set("aac_coder", "fast");

            let audio = audio
                .open_as_with(codec, aac_opts)
                .context("Failed to open AAC encoder")?;

            stream.set_time_base(audio_time_base);
            stream.set_parameters(&audio);

            audio_stream_index = Some(stream_index);
            audio_encoder = Some(audio);
        }

        info!("Created native muxer for {:?}", output_path);

        Ok(Self {
            format_context,
            video_stream_index,
            audio_stream_index,
            audio_encoder,
            output_path: output_path.to_path_buf(),
            video_time_base,
            video_frame_rate: rounded_fps,
            audio_time_base,
            expect_audio: config.expect_audio,
            faststart: config.faststart,
        })
    }

    /// Writes encoded video and audio packets to the MP4 file.
    ///
    /// This method handles:
    /// 1. Calculating a common base timestamp (QPC-based) to normalize all packets to start at 0.
    /// 2. Normalizing and interleaved writing of video and audio packets.
    /// 3. Normalizing PTS/DTS values for the target MP4 timebase.
    /// 4. Handling fragmented MP4 (+frag_keyframe) to minimize memory usage during save.
    ///
    /// # Arguments
    ///
    /// * `video_packets` - Slice of video packets to write.
    /// * `audio_packets` - Slice of audio packets to write.
    ///
    /// # Returns
    ///
    /// Tuple of (video_count, audio_count) written.
    pub fn write_packets(
        &mut self,
        video_packets: &[&EncodedPacket],
        audio_packets: &[&EncodedPacket],
    ) -> Result<(usize, usize)> {
        if video_packets.is_empty() {
            anyhow::bail!("No video packets to write");
        }

        let qpc_freq = crate::buffer::ring::qpc_frequency().max(1);

        let video_start_qpc = video_packets
            .iter()
            .filter(|packet| !is_parameter_set_payload(packet.data.as_ref()))
            .map(|packet| packet.dts)
            .min()
            .or_else(|| video_packets.iter().map(|packet| packet.dts).min())
            .unwrap_or(0);

        let video_end_qpc = {
            let max_video_pts = video_packets
                .iter()
                .map(|packet| packet.pts)
                .max()
                .unwrap_or(0);
            let frame_duration = default_video_frame_qpc(self.video_frame_rate);
            max_video_pts.saturating_add(frame_duration)
        };

        let min_audio_qpc = audio_packets.iter().map(|packet| packet.pts).min();

        let base_qpc = std::iter::once(video_start_qpc)
            .chain(min_audio_qpc)
            .min()
            .unwrap_or(0);

        let mut options = ffmpeg::Dictionary::new();
        if self.faststart {
            options.set("movflags", "+faststart");
        }
        self.format_context
            .write_header_with(options)
            .context("Failed to write MP4 header")?;

        let encoded_audio: Vec<EncodedAudioPacket> = if let (Some(_), Some(audio_encoder)) =
            (self.audio_stream_index, self.audio_encoder.as_mut())
        {
            if audio_packets.is_empty() && !self.expect_audio {
                vec![]
            } else {
                mix_and_encode_audio_chunks(audio_encoder, audio_packets, base_qpc, video_end_qpc)?
            }
        } else {
            vec![]
        };

        // Log video stream info.
        {
            let qpc_freq = crate::buffer::ring::qpc_frequency();
            let min_pts = video_packets.iter().map(|p| p.pts).min().unwrap_or(0);
            let max_pts = video_packets.iter().map(|p| p.pts).max().unwrap_or(0);
            let duration_ms = if qpc_freq > 0 {
                max_pts.saturating_sub(min_pts) * 1000 / qpc_freq
            } else {
                0
            };
            let keyframe_count = video_packets.iter().filter(|p| p.is_keyframe).count();
            let actual_fps = if duration_ms > 0 {
                (video_packets.len() as i64 * 1000 / duration_ms) as i32
            } else {
                0
            };
            info!(
                "Writing {} video packets: duration={}ms, keyframes={}, expected_fps={}, actual_fps={}",
                video_packets.len(),
                duration_ms,
                keyframe_count,
                self.video_frame_rate,
                actual_fps
            );
        }

        // Merge-sort video and audio by decode time (DTS) and write each packet.
        // This aligns with FFmpeg's expectations for fragmented MP4 output.
        let mut aac_iter = encoded_audio.iter().peekable();
        let audio_stream_idx = self.audio_stream_index;
        let audio_time_base = self.audio_time_base;
        let mut video_count = 0usize;
        let mut audio_count = 0usize;

        let mut video_packets_ordered: Vec<&EncodedPacket> = video_packets.to_vec();
        video_packets_ordered.sort_by_key(|pkt| (pkt.dts, pkt.pts));

        let default_duration = qpc_to_time_base(
            default_video_frame_qpc(self.video_frame_rate),
            self.video_time_base.1 as i64,
        )
        .max(1);

        for pkt in &video_packets_ordered {
            // Flush AAC packets whose PTS/DTS is at or before this video packet.
            let video_us = pkt.dts.saturating_sub(base_qpc).saturating_mul(1_000_000) / qpc_freq;

            while let Some(audio_packet) = aac_iter.peek() {
                let audio_us =
                    audio_packet.pts.saturating_mul(1_000_000) / AUDIO_SAMPLE_RATE as i64;
                if audio_us > video_us {
                    break;
                }
                let Some(audio_packet) = aac_iter.next() else {
                    break;
                };
                if let Some(aidx) = audio_stream_idx {
                    write_audio_frame_direct(
                        &mut self.format_context,
                        aidx,
                        &audio_packet.data,
                        audio_packet.pts,
                        audio_packet.duration,
                        audio_time_base,
                    )?;
                    audio_count += 1;
                }
            }

            // Write video packet.
            let pts = qpc_to_time_base(
                pkt.pts.saturating_sub(base_qpc),
                self.video_time_base.1 as i64,
            );
            let dts = qpc_to_time_base(
                pkt.dts.saturating_sub(base_qpc),
                self.video_time_base.1 as i64,
            );

            write_borrowed_video_packet(
                &mut self.format_context,
                self.video_stream_index,
                pkt,
                pts.max(0),
                dts.max(0),
                default_duration,
            )?;
            video_count += 1;
        }

        // Flush any remaining AAC packets that come after the last video frame.
        for audio_packet in aac_iter {
            if let Some(aidx) = audio_stream_idx {
                write_audio_frame_direct(
                    &mut self.format_context,
                    aidx,
                    &audio_packet.data,
                    audio_packet.pts,
                    audio_packet.duration,
                    audio_time_base,
                )?;
                audio_count += 1;
            }
        }
        // aac_iter is consumed by the for loop above.
        // encoded_audio goes out of scope after video_packets_ordered is dropped below.

        // Release video packet reference Vec — all data has been written to the file.
        drop(video_packets_ordered);

        crate::output::saver::log_save_memory("before write_trailer", None, None);
        self.format_context.write_trailer()?;
        crate::output::saver::log_save_memory("after write_trailer", None, None);

        // Explicitly drop audio encoder to free FFmpeg resources early
        let _ = self.audio_encoder.take();
        crate::output::saver::log_save_memory("after audio_encoder drop", None, None);

        info!(
            "Muxed {} video packets and {} audio packets to {:?}",
            video_count, audio_count, self.output_path
        );

        Ok((video_count, audio_count))
    }
}

fn write_borrowed_video_packet(
    format_context: &mut ffmpeg::format::context::Output,
    stream_index: usize,
    packet: &EncodedPacket,
    pts: i64,
    dts: i64,
    duration: i64,
) -> Result<()> {
    unsafe {
        let mut raw_packet: ffmpeg::ffi::AVPacket = std::mem::zeroed();
        ffmpeg::ffi::av_init_packet(&mut raw_packet);
        raw_packet.data = packet.data.as_ptr() as *mut u8;
        raw_packet.size = packet.data.len() as i32;
        raw_packet.stream_index = stream_index as i32;
        raw_packet.pts = pts;
        raw_packet.dts = dts;
        raw_packet.duration = duration;
        raw_packet.pos = -1;

        if packet.is_keyframe {
            raw_packet.flags |= ffmpeg::ffi::AV_PKT_FLAG_KEY;
        }

        match ffmpeg::ffi::av_interleaved_write_frame(format_context.as_mut_ptr(), &mut raw_packet)
        {
            0 => Ok(()),
            err => Err(ffmpeg::Error::from(err).into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode::StreamType;

    fn qpc_for_frame_index(target_frame_index: usize) -> i64 {
        let qpc_freq = crate::buffer::ring::qpc_frequency().max(1);
        let approx = ((target_frame_index as i128) * (qpc_freq as i128)
            / (AUDIO_SAMPLE_RATE as i128)) as i64;

        for delta in approx.saturating_sub(4096)..=approx.saturating_add(4096) {
            if qpc_to_sample_index(delta) == target_frame_index {
                return delta;
            }
        }

        approx
    }

    fn packet_from_i16_samples(samples: &[i16], pts: i64, stream: StreamType) -> EncodedPacket {
        let data: Vec<u8> = samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        EncodedPacket::new(data, pts, pts, false, stream)
    }

    #[test]
    fn mix_audio_packets_to_pcm_snaps_small_packet_jitter() {
        let first = packet_from_i16_samples(
            &[1000, 1000, 1001, 1001, 1002, 1002, 1003, 1003],
            0,
            StreamType::Microphone,
        );
        let second_pts = qpc_for_frame_index(5);
        assert_eq!(qpc_to_sample_index(second_pts), 5);
        let second = packet_from_i16_samples(
            &[2000, 2000, 2001, 2001, 2002, 2002, 2003, 2003],
            second_pts,
            StreamType::Microphone,
        );

        let audio_packets = vec![&first, &second];
        let video_end = qpc_for_frame_index(8);
        let mixed = mix_audio_packets_to_pcm(&audio_packets, 0, video_end);

        assert_eq!(mixed.len(), 16);
        assert_eq!(
            &mixed[..16],
            &[
                1000, 1000, 1001, 1001, 1002, 1002, 1003, 1003, 2000, 2000, 2001, 2001, 2002, 2002,
                2003, 2003
            ]
        );
    }

    #[test]
    fn mix_audio_packets_to_pcm_preserves_real_gaps() {
        let first = packet_from_i16_samples(
            &[1000, 1000, 1001, 1001, 1002, 1002, 1003, 1003],
            0,
            StreamType::Microphone,
        );
        let second_start_frame = 4 + AUDIO_PACKET_JITTER_TOLERANCE_FRAMES + 4;
        let second_pts = qpc_for_frame_index(second_start_frame);
        assert_eq!(qpc_to_sample_index(second_pts), second_start_frame);
        let second = packet_from_i16_samples(
            &[2000, 2000, 2001, 2001, 2002, 2002, 2003, 2003],
            second_pts,
            StreamType::Microphone,
        );

        let audio_packets = vec![&first, &second];
        let video_end = qpc_for_frame_index(second_start_frame + 4);
        let mixed = mix_audio_packets_to_pcm(&audio_packets, 0, video_end);

        let gap_start = 4 * AUDIO_CHANNELS as usize;
        let gap_end = second_start_frame * AUDIO_CHANNELS as usize;
        assert!(mixed[gap_start..gap_end].iter().all(|&sample| sample == 0));
        assert_eq!(
            &mixed[gap_end..gap_end + 8],
            &[2000, 2000, 2001, 2001, 2002, 2002, 2003, 2003]
        );
    }

    #[test]
    fn mix_audio_packets_to_pcm_truncates_audio_past_video_end() {
        let first = packet_from_i16_samples(
            &[1000, 1000, 1001, 1001, 1002, 1002, 1003, 1003],
            0,
            StreamType::Microphone,
        );
        let late_pts = qpc_for_frame_index(48);
        assert_eq!(qpc_to_sample_index(late_pts), 48);
        let late = packet_from_i16_samples(
            &[2000, 2000, 2001, 2001, 2002, 2002, 2003, 2003],
            late_pts,
            StreamType::Microphone,
        );

        let audio_packets = vec![&first, &late];
        let video_end = qpc_for_frame_index(16);
        let mixed = mix_audio_packets_to_pcm(&audio_packets, 0, video_end);

        assert_eq!(mixed.len(), 16 * AUDIO_CHANNELS as usize);
        assert_eq!(
            &mixed[..8],
            &[1000, 1000, 1001, 1001, 1002, 1002, 1003, 1003]
        );
        assert!(mixed[8..].iter().all(|&sample| sample == 0));
    }

    #[test]
    fn mix_audio_packets_places_audio_relative_to_base() {
        let audio_start_frame = 500 * 48000 / 1000;
        let first = packet_from_i16_samples(
            &[1000, 1000, 1001, 1001, 1002, 1002, 1003, 1003],
            qpc_for_frame_index(audio_start_frame),
            StreamType::Microphone,
        );
        let audio_packets = vec![&first];

        let video_end_frames = audio_start_frame + 8;
        let video_end = qpc_for_frame_index(video_end_frames);
        let mixed = mix_audio_packets_to_pcm(&audio_packets, 0, video_end);

        let audio_start_samples = audio_start_frame * AUDIO_CHANNELS as usize;
        assert!(mixed[..audio_start_samples]
            .iter()
            .all(|&sample| sample == 0));
        assert_eq!(
            &mixed[audio_start_samples..audio_start_samples + 8],
            &[1000, 1000, 1001, 1001, 1002, 1002, 1003, 1003]
        );
    }

    #[test]
    fn mix_audio_packets_pads_when_audio_starts_after_video() {
        let audio_start_frame = 100;
        let first = packet_from_i16_samples(
            &[1000, 1000, 1001, 1001, 1002, 1002, 1003, 1003],
            qpc_for_frame_index(audio_start_frame),
            StreamType::Microphone,
        );
        let audio_packets = vec![&first];
        let video_end = qpc_for_frame_index(audio_start_frame + 8);
        let mixed = mix_audio_packets_to_pcm(&audio_packets, 0, video_end);

        let padding_samples = audio_start_frame * AUDIO_CHANNELS as usize;
        assert!(mixed[..padding_samples].iter().all(|&sample| sample == 0));
        assert_eq!(
            &mixed[padding_samples..padding_samples + 8],
            &[1000, 1000, 1001, 1001, 1002, 1002, 1003, 1003]
        );
    }
}

fn default_video_frame_qpc(video_frame_rate: i32) -> i64 {
    let qpc_freq = crate::buffer::ring::qpc_frequency();
    (qpc_freq / video_frame_rate.max(1) as i64).max(1)
}

fn qpc_to_time_base(delta_qpc: i64, time_base_den: i64) -> i64 {
    let qpc_freq = crate::buffer::ring::qpc_frequency();
    if qpc_freq <= 0 {
        return 0;
    }
    ((delta_qpc as i128) * (time_base_den as i128) / (qpc_freq as i128)) as i64
}

fn qpc_to_sample_index(delta_qpc: i64) -> usize {
    let qpc_freq = crate::buffer::ring::qpc_frequency();
    if qpc_freq <= 0 || delta_qpc <= 0 {
        return 0;
    }
    ((delta_qpc as i128) * (AUDIO_SAMPLE_RATE as i128) / (qpc_freq as i128)) as usize
}

fn is_parameter_set_payload(data: &[u8]) -> bool {
    if let Some(32..=34) = crate::buffer::ring::hevc_nal_type(data) {
        return true;
    }

    matches!(crate::buffer::ring::h264_nal_type(data), Some(7 | 8))
}

fn audio_stream_id(packet: &EncodedPacket) -> u8 {
    match packet.stream {
        crate::encode::StreamType::SystemAudio => 1,
        crate::encode::StreamType::Microphone => 2,
        _ => 0,
    }
}

struct AudioPacketPlacement<'a> {
    packet: &'a EncodedPacket,
    start_index: usize,
}

fn compute_audio_placements<'a>(
    audio_packets: &[&'a EncodedPacket],
    base_qpc: i64,
) -> Vec<AudioPacketPlacement<'a>> {
    let mut stream_next_indices: std::collections::HashMap<u8, usize> =
        std::collections::HashMap::new();
    let mut ordered_audio_packets: Vec<&EncodedPacket> = audio_packets.to_vec();
    ordered_audio_packets.sort_by_key(|packet| (audio_stream_id(packet), packet.pts));

    let mut placements: Vec<AudioPacketPlacement> = Vec::with_capacity(ordered_audio_packets.len());

    for packet in ordered_audio_packets {
        let stream_id = audio_stream_id(packet);
        let start_frames = qpc_to_sample_index(packet.pts.saturating_sub(base_qpc));
        let nominal_start_index = start_frames.saturating_mul(AUDIO_CHANNELS as usize);

        let samples_len = packet.data.len() / 2;
        if samples_len == 0 {
            continue;
        }

        let start_index = match stream_next_indices.get(&stream_id).copied() {
            Some(next_index)
                if nominal_start_index.abs_diff(next_index)
                    <= AUDIO_PACKET_JITTER_TOLERANCE_FRAMES
                        .saturating_mul(AUDIO_CHANNELS as usize) =>
            {
                next_index
            }
            _ => nominal_start_index,
        };

        placements.push(AudioPacketPlacement {
            packet,
            start_index,
        });

        stream_next_indices.insert(stream_id, start_index.saturating_add(samples_len));
    }

    placements.sort_by_key(|p| p.start_index);
    placements
}

#[cfg(test)]
fn mix_audio_packets_to_pcm(
    audio_packets: &[&EncodedPacket],
    base_qpc: i64,
    video_end_qpc: i64,
) -> Vec<i16> {
    let placements = compute_audio_placements(audio_packets, base_qpc);

    let video_duration_qpc = video_end_qpc.saturating_sub(base_qpc);
    let video_required_samples =
        qpc_to_sample_index(video_duration_qpc).saturating_mul(AUDIO_CHANNELS as usize);
    let final_len = video_required_samples.max(1);

    let mut mixed = vec![0_i32; final_len];
    for p in placements {
        let p_start = p.start_index;
        let p_len = p.packet.data.len() / 2;
        let p_end = p_start + p_len;

        let overlap_end = p_end.min(final_len);
        let len = overlap_end.saturating_sub(p_start);

        let data = p.packet.data.as_ref();
        for i in 0..len {
            let data_idx = i * 2;
            let sample = i16::from_le_bytes([data[data_idx], data[data_idx + 1]]) as i32;
            mixed[p_start + i] = mixed[p_start + i].saturating_add(sample);
        }
    }

    mixed
        .into_iter()
        .map(|sample| {
            let limit = 24000.0;
            let sample_f32 = sample as f32;
            let clipped = if sample_f32 > limit {
                limit + (sample_f32 - limit) / (1.0 + (sample_f32 - limit) / (32767.0 - limit))
            } else if sample_f32 < -limit {
                -limit + (sample_f32 + limit) / (1.0 - (sample_f32 + limit) / (32768.0 - limit))
            } else {
                sample_f32
            };
            clipped.clamp(-32768.0, 32767.0).round() as i16
        })
        .collect()
}

fn copy_pcm_into_frame(frame: &mut ffmpeg::frame::Audio, chunk: &[i16]) {
    match frame.format() {
        ffmpeg::format::Sample::I16(ffmpeg::format::sample::Type::Packed) => {
            let plane = frame.plane_mut::<(i16, i16)>(0);
            for (dst, src) in plane
                .iter_mut()
                .zip(chunk.chunks_exact(AUDIO_CHANNELS as usize))
            {
                *dst = (src[0], src[1]);
            }
        }
        ffmpeg::format::Sample::I16(ffmpeg::format::sample::Type::Planar) => {
            let mut left = Vec::with_capacity(chunk.len() / AUDIO_CHANNELS as usize);
            let mut right = Vec::with_capacity(chunk.len() / AUDIO_CHANNELS as usize);
            for src in chunk.chunks_exact(AUDIO_CHANNELS as usize) {
                left.push(src[0]);
                right.push(src[1]);
            }
            write_i16_plane(frame.data_mut(0), &left);
            write_i16_plane(frame.data_mut(1), &right);
        }
        ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Packed) => {
            let plane = frame.plane_mut::<(f32, f32)>(0);
            for (dst, src) in plane
                .iter_mut()
                .zip(chunk.chunks_exact(AUDIO_CHANNELS as usize))
            {
                *dst = (
                    src[0] as f32 / i16::MAX as f32,
                    src[1] as f32 / i16::MAX as f32,
                );
            }
        }
        ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Planar) => {
            let mut left = Vec::with_capacity(chunk.len() / AUDIO_CHANNELS as usize);
            let mut right = Vec::with_capacity(chunk.len() / AUDIO_CHANNELS as usize);
            for src in chunk.chunks_exact(AUDIO_CHANNELS as usize) {
                left.push(src[0] as f32 / i16::MAX as f32);
                right.push(src[1] as f32 / i16::MAX as f32);
            }
            write_f32_plane(frame.data_mut(0), &left);
            write_f32_plane(frame.data_mut(1), &right);
        }
        _ => {
            let packed_bytes = frame.data_mut(0);
            for (index, sample) in chunk.iter().enumerate() {
                let start = index * PCM_BYTES_PER_SAMPLE;
                if start + PCM_BYTES_PER_SAMPLE <= packed_bytes.len() {
                    packed_bytes[start..start + PCM_BYTES_PER_SAMPLE]
                        .copy_from_slice(&sample.to_le_bytes());
                }
            }
        }
    }
}

fn write_i16_plane(dst: &mut [u8], samples: &[i16]) {
    for (index, sample) in samples.iter().enumerate() {
        let start = index * PCM_BYTES_PER_SAMPLE;
        if start + PCM_BYTES_PER_SAMPLE <= dst.len() {
            dst[start..start + PCM_BYTES_PER_SAMPLE].copy_from_slice(&sample.to_le_bytes());
        }
    }
}

fn write_f32_plane(dst: &mut [u8], samples: &[f32]) {
    for (index, sample) in samples.iter().enumerate() {
        let start = index * std::mem::size_of::<f32>();
        if start + std::mem::size_of::<f32>() <= dst.len() {
            dst[start..start + std::mem::size_of::<f32>()].copy_from_slice(&sample.to_ne_bytes());
        }
    }
}

fn mix_and_encode_audio_chunks(
    encoder: &mut ffmpeg::encoder::Audio,
    audio_packets: &[&EncodedPacket],
    base_qpc: i64,
    video_end_qpc: i64,
) -> Result<Vec<EncodedAudioPacket>> {
    let placements = compute_audio_placements(audio_packets, base_qpc);

    let video_duration_qpc = video_end_qpc.saturating_sub(base_qpc);
    let video_required_samples =
        qpc_to_sample_index(video_duration_qpc).saturating_mul(AUDIO_CHANNELS as usize);
    let final_len = video_required_samples.max(1);

    let mut result = Vec::new();

    let input_format = ffmpeg::format::Sample::I16(ffmpeg::format::sample::Type::Packed);
    let channel_layout = ffmpeg::channel_layout::ChannelLayout::STEREO;
    let mut resampler = ffmpeg::software::resampling::Context::get(
        input_format,
        channel_layout,
        AUDIO_SAMPLE_RATE,
        encoder.format(),
        encoder.channel_layout(),
        encoder.rate(),
    )
    .context("Failed to create audio resampler")?;

    let frame_size = encoder.frame_size().max(1024) as usize;
    let samples_per_chunk = frame_size.saturating_mul(AUDIO_CHANNELS as usize);
    let mut next_pts = 0i64;

    // Process in ~1-second chunks to bound working set memory
    let chunk_samples = AUDIO_SAMPLE_RATE as usize * AUDIO_CHANNELS as usize; // 1 second
    let mut chunk_start = 0;
    let mut search_idx = 0;

    let mut mixed_i32 = vec![0_i32; chunk_samples];
    let mut mixed_i16 = vec![0_i16; chunk_samples];
    let mut pcm_buffer = Vec::with_capacity(samples_per_chunk * 2);

    while chunk_start < final_len {
        let chunk_end = (chunk_start + chunk_samples).min(final_len);
        let current_chunk_len = chunk_end - chunk_start;

        mixed_i32[..current_chunk_len].fill(0);

        while search_idx < placements.len() {
            let p = &placements[search_idx];
            let p_len = p.packet.data.len() / 2;
            if p.start_index + p_len <= chunk_start {
                search_idx += 1;
            } else {
                break;
            }
        }

        for p in placements.iter().skip(search_idx) {
            let p_start = p.start_index;
            if p_start >= chunk_end {
                break;
            }

            let p_len = p.packet.data.len() / 2;
            let p_end = p_start + p_len;
            if p_end <= chunk_start {
                continue;
            }

            let overlap_start = p_start.max(chunk_start);
            let overlap_end = p_end.min(chunk_end);

            let packet_offset = overlap_start - p_start;
            let chunk_offset = overlap_start - chunk_start;
            let len = overlap_end - overlap_start;

            let data = p.packet.data.as_ref();
            for j in 0..len {
                let data_idx = (packet_offset + j) * 2;
                let sample = i16::from_le_bytes([data[data_idx], data[data_idx + 1]]) as i32;
                mixed_i32[chunk_offset + j] = mixed_i32[chunk_offset + j].saturating_add(sample);
            }
        }

        for (i, &sample) in mixed_i32[..current_chunk_len].iter().enumerate() {
            let limit = 24000.0;
            let sample_f32 = sample as f32;
            let clipped = if sample_f32 > limit {
                limit + (sample_f32 - limit) / (1.0 + (sample_f32 - limit) / (32767.0 - limit))
            } else if sample_f32 < -limit {
                -limit + (sample_f32 + limit) / (1.0 - (sample_f32 + limit) / (32768.0 - limit))
            } else {
                sample_f32
            };
            mixed_i16[i] = clipped.clamp(-32768.0, 32767.0).round() as i16;
        }

        pcm_buffer.extend_from_slice(&mixed_i16[..current_chunk_len]);

        let mut offset = 0usize;
        while offset + samples_per_chunk <= pcm_buffer.len() {
            let chunk = &pcm_buffer[offset..offset + samples_per_chunk];
            offset += samples_per_chunk;

            let samples_in_frame = (chunk.len() / AUDIO_CHANNELS as usize).max(1);
            let mut input =
                ffmpeg::frame::Audio::new(input_format, samples_in_frame, channel_layout);
            input.set_rate(AUDIO_SAMPLE_RATE);
            input.set_pts(Some(next_pts));
            copy_pcm_into_frame(&mut input, chunk);

            let mut converted = ffmpeg::frame::Audio::empty();
            resampler
                .run(&input, &mut converted)
                .context("Failed to resample audio frame")?;
            converted.set_pts(Some(next_pts));
            next_pts = next_pts.saturating_add(converted.samples() as i64);

            encoder
                .send_frame(&converted)
                .context("Failed to send audio frame to encoder")?;
            drain_encoder_to_vec(encoder, &mut result);
        }

        if offset > 0 {
            pcm_buffer.drain(..offset);
            // Aggressively release capacity when it significantly exceeds current length.
            // This prevents the allocator from holding onto large blocks across chunks.
            if pcm_buffer.capacity() > pcm_buffer.len() * 4 && pcm_buffer.capacity() > 16384 {
                pcm_buffer.shrink_to_fit();
            }
        }

        chunk_start += current_chunk_len;
    }

    if !pcm_buffer.is_empty() {
        let chunk = &pcm_buffer;
        let samples_in_frame = (chunk.len() / AUDIO_CHANNELS as usize).max(1);
        let mut input = ffmpeg::frame::Audio::new(input_format, samples_in_frame, channel_layout);
        input.set_rate(AUDIO_SAMPLE_RATE);
        input.set_pts(Some(next_pts));
        copy_pcm_into_frame(&mut input, chunk);

        let mut converted = ffmpeg::frame::Audio::empty();
        resampler
            .run(&input, &mut converted)
            .context("Failed to resample audio frame")?;
        converted.set_pts(Some(next_pts));

        encoder
            .send_frame(&converted)
            .context("Failed to send audio frame to encoder")?;
        drain_encoder_to_vec(encoder, &mut result);
    }

    encoder.send_eof().ok();
    drain_encoder_to_vec(encoder, &mut result);

    // Release working set buffers — they are no longer needed.
    drop(pcm_buffer);
    drop(mixed_i16);
    drop(mixed_i32);
    drop(placements);

    // Shrink result to exact size — no extra capacity after encoding completes.
    result.shrink_to_fit();

    Ok(result)
}

/// Drains encoded AAC packets from `encoder` into `result`.
/// Negative-PTS priming frames (AAC encoder delay artifact) are skipped.
fn drain_encoder_to_vec(
    encoder: &mut ffmpeg::encoder::Audio,
    result: &mut Vec<EncodedAudioPacket>,
) {
    let mut packet = ffmpeg::Packet::empty();
    while encoder.receive_packet(&mut packet).is_ok() {
        if let Some(pts) = packet.pts() {
            if pts >= 0 {
                let data = bytes::Bytes::copy_from_slice(packet.data().unwrap_or(&[]));
                let duration = packet.duration().max(encoder.frame_size() as i64).max(1);
                result.push(EncodedAudioPacket {
                    data,
                    pts,
                    duration,
                });
            }
        }
        packet = ffmpeg::Packet::empty();
    }
}

/// Writes a single raw AAC packet to the muxer via av_write_frame.
/// `pts` is in audio sample units (audio time base denominator = AUDIO_SAMPLE_RATE).
fn write_audio_frame_direct(
    format_context: &mut ffmpeg::format::context::Output,
    stream_index: usize,
    data: &bytes::Bytes,
    pts: i64,
    duration: i64,
    _stream_time_base: (i32, i32),
) -> Result<()> {
    unsafe {
        let mut raw_packet: ffmpeg::ffi::AVPacket = std::mem::zeroed();
        ffmpeg::ffi::av_init_packet(&mut raw_packet);
        raw_packet.data = data.as_ptr() as *mut u8;
        raw_packet.size = data.len() as i32;
        raw_packet.stream_index = stream_index as i32;
        raw_packet.pts = pts;
        raw_packet.dts = pts;
        raw_packet.duration = duration.max(1);
        raw_packet.pos = -1;
        match ffmpeg::ffi::av_interleaved_write_frame(format_context.as_mut_ptr(), &mut raw_packet)
        {
            0 => Ok(()),
            err => Err(ffmpeg::Error::from(err).into()),
        }
    }
}
