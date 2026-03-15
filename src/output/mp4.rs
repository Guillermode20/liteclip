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

            let audio = audio.open_as(codec).context("Failed to open AAC encoder")?;

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

        let base_qpc = video_packets
            .first()
            .map(|packet| packet.pts)
            .into_iter()
            .chain(audio_packets.first().map(|packet| packet.pts))
            .min()
            .unwrap_or(0);
        let video_end_qpc =
            estimate_video_end_qpc(video_packets, self.video_frame_rate, self.video_time_base.1);

        let mut options = ffmpeg::Dictionary::new();
        // +frag_keyframe avoids FFmpeg's delay_moov packet queue, which would
        // otherwise copy every packet in RAM until the final moov box can be
        // written. Keep the output fragmented here so save-time memory stays
        // flat; converting back to a regular MP4 at trailer time reintroduces
        // a large whole-file rewrite and RAM spike.
        options.set("movflags", "+frag_keyframe");
        self.format_context
            .write_header_with(options)
            .context("Failed to write MP4 header")?;

        // Encode all audio to AAC BEFORE writing any video.
        // With fragmented MP4, packets from all streams must be written in
        // interleaved DTS order. Writing all video first then all audio creates
        // video-only fragments and causes av_write_trailer to fail.
        // By encoding audio up-front we can merge video + audio by time and
        // feed them to av_write_frame in the correct order.
        let encoded_audio: Vec<EncodedAudioPacket> = if let (Some(_), Some(audio_encoder)) =
            (self.audio_stream_index, self.audio_encoder.as_mut())
        {
            let mixed_pcm = mix_audio_packets_to_pcm(audio_packets, base_qpc, video_end_qpc);
            if mixed_pcm.is_empty() && !self.expect_audio {
                vec![]
            } else {
                encode_audio_to_vec(audio_encoder, &mixed_pcm)?
            }
        } else {
            vec![]
        };

        // Log video stream info.
        {
            let qpc_freq = crate::buffer::ring::qpc_frequency();
            let first_pts = video_packets.first().map(|p| p.pts).unwrap_or(0);
            let last_pts = video_packets.last().map(|p| p.pts).unwrap_or(0);
            let duration_ms = if qpc_freq > 0 {
                last_pts.saturating_sub(first_pts) * 1000 / qpc_freq
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

        // Merge-sort video and audio by presentation time, writing each packet via
        // av_write_frame (non-interleaved, since we handle the ordering ourselves).
        // +frag_keyframe flushes a new fragment to disk at each video keyframe.
        let qpc_freq = crate::buffer::ring::qpc_frequency().max(1);
        let mut aac_iter = encoded_audio.iter().peekable();
        let audio_stream_idx = self.audio_stream_index;
        let audio_time_base = self.audio_time_base;
        let mut video_count = 0usize;
        let mut audio_count = 0usize;

        for (idx, pkt) in video_packets.iter().enumerate() {
            // Flush AAC packets whose PTS is at or before this video packet.
            let video_us = pkt.pts.saturating_sub(base_qpc).saturating_mul(1_000_000) / qpc_freq;

            while let Some(audio_packet) = aac_iter.peek() {
                let audio_us =
                    audio_packet.pts.saturating_mul(1_000_000) / AUDIO_SAMPLE_RATE as i64;
                if audio_us > video_us {
                    break;
                }
                let audio_packet = aac_iter.next().unwrap();
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
            let next_pts = video_packets
                .get(idx + 1)
                .map(|n| n.pts)
                .unwrap_or_else(|| {
                    pkt.pts
                        .saturating_add(default_video_frame_qpc(self.video_frame_rate))
                });
            let duration_qpc = next_pts.saturating_sub(pkt.pts).max(0);
            let duration = qpc_to_time_base(duration_qpc, self.video_time_base.1 as i64).max(1);

            write_borrowed_video_packet(
                &mut self.format_context,
                self.video_stream_index,
                pkt,
                pts.max(0),
                dts.max(0),
                duration,
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

        self.format_context.write_trailer()?;
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

        // av_write_frame bypasses the interleaver since we merge video and audio
        // in time order in the caller. +frag_keyframe still triggers a fragment
        // flush at each keyframe via ff_mov_write_packet inside the muxer.
        match ffmpeg::ffi::av_write_frame(format_context.as_mut_ptr(), &mut raw_packet) {
            0 => Ok(()),
            err => Err(ffmpeg::Error::from(err).into()),
        }
    }
}

fn default_video_frame_qpc(video_frame_rate: i32) -> i64 {
    let qpc_freq = crate::buffer::ring::qpc_frequency();
    (qpc_freq / video_frame_rate.max(1) as i64).max(1)
}

fn estimate_video_end_qpc(
    video_packets: &[&EncodedPacket],
    video_frame_rate: i32,
    video_tb_den: i32,
) -> i64 {
    video_packets
        .last()
        .map(|packet| {
            let frame_qpc = default_video_frame_qpc(video_frame_rate)
                .max(qpc_to_qpc(1, video_tb_den as i64).max(1));
            packet.pts.saturating_add(frame_qpc)
        })
        .unwrap_or(0)
}

fn qpc_to_time_base(delta_qpc: i64, time_base_den: i64) -> i64 {
    let qpc_freq = crate::buffer::ring::qpc_frequency();
    if qpc_freq <= 0 {
        return 0;
    }
    ((delta_qpc as i128) * (time_base_den as i128) / (qpc_freq as i128)) as i64
}

fn qpc_to_qpc(delta: i64, source_den: i64) -> i64 {
    if source_den <= 0 {
        return 0;
    }
    let qpc_freq = crate::buffer::ring::qpc_frequency();
    ((delta as i128) * (qpc_freq as i128) / (source_den as i128)) as i64
}

fn qpc_to_sample_index(delta_qpc: i64) -> usize {
    let qpc_freq = crate::buffer::ring::qpc_frequency();
    if qpc_freq <= 0 || delta_qpc <= 0 {
        return 0;
    }
    ((delta_qpc as i128) * (AUDIO_SAMPLE_RATE as i128) / (qpc_freq as i128)) as usize
}

fn mix_audio_packets_to_pcm(
    audio_packets: &[&EncodedPacket],
    base_qpc: i64,
    video_end_qpc: i64,
) -> Vec<i16> {
    let mut mixed = Vec::<i32>::new();

    for packet in audio_packets {
        let start_frames = qpc_to_sample_index(packet.pts.saturating_sub(base_qpc));
        let start_index = start_frames.saturating_mul(AUDIO_CHANNELS as usize);

        let samples: Vec<i16> = packet
            .data
            .chunks_exact(2)
            .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();

        if samples.is_empty() {
            continue;
        }

        let required_len = start_index.saturating_add(samples.len());
        if mixed.len() < required_len {
            mixed.resize(required_len, 0);
        }

        for (offset, sample) in samples.into_iter().enumerate() {
            let slot = &mut mixed[start_index + offset];
            *slot = slot.saturating_add(sample as i32);
        }
    }

    let video_required_samples = qpc_to_sample_index(video_end_qpc.saturating_sub(base_qpc))
        .saturating_mul(AUDIO_CHANNELS as usize);
    if mixed.len() < video_required_samples {
        mixed.resize(video_required_samples, 0);
    }

    mixed
        .into_iter()
        .map(|sample| sample.clamp(i16::MIN as i32, i16::MAX as i32) as i16)
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

fn encode_audio_to_vec(
    encoder: &mut ffmpeg::encoder::Audio,
    pcm_samples: &[i16],
) -> Result<Vec<EncodedAudioPacket>> {
    let input_format = ffmpeg::format::Sample::I16(ffmpeg::format::sample::Type::Packed);
    let channel_layout = ffmpeg::channel_layout::ChannelLayout::STEREO;
    let frame_size = encoder.frame_size().max(1024) as usize;
    let samples_per_chunk = frame_size.saturating_mul(AUDIO_CHANNELS as usize);
    let mut result: Vec<EncodedAudioPacket> = Vec::new();
    let mut next_pts = 0i64;
    let mut resampler = ffmpeg::software::resampling::Context::get(
        input_format,
        channel_layout,
        AUDIO_SAMPLE_RATE,
        encoder.format(),
        encoder.channel_layout(),
        encoder.rate(),
    )
    .context("Failed to create audio resampler")?;

    let mut offset = 0usize;
    while offset < pcm_samples.len() {
        let end = (offset + samples_per_chunk).min(pcm_samples.len());
        let chunk = &pcm_samples[offset..end];
        offset = end;

        if chunk.is_empty() {
            continue;
        }

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
        next_pts = next_pts.saturating_add(converted.samples() as i64);

        encoder
            .send_frame(&converted)
            .context("Failed to send audio frame to encoder")?;
        drain_encoder_to_vec(encoder, &mut result);
    }

    encoder.send_eof().ok();
    drain_encoder_to_vec(encoder, &mut result);
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
        match ffmpeg::ffi::av_write_frame(format_context.as_mut_ptr(), &mut raw_packet) {
            0 => Ok(()),
            err => Err(ffmpeg::Error::from(err).into()),
        }
    }
}
