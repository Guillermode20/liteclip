use crate::clip::muxer::{
    functions::{AUDIO_CHANNELS, AUDIO_SAMPLE_RATE},
    MuxerConfig,
};
use crate::encode::EncodedPacket;
use anyhow::{Context, Result};
use ffmpeg_next as ffmpeg;
use std::path::{Path, PathBuf};
use tracing::info;

const AUDIO_BITRATE_BPS: usize = 192_000;
const PCM_BYTES_PER_SAMPLE: usize = 2;

pub struct FfmpegMuxer {
    format_context: ffmpeg::format::context::Output,
    video_stream_index: usize,
    audio_stream_index: Option<usize>,
    audio_encoder: Option<ffmpeg::encoder::Audio>,
    output_path: PathBuf,
    video_time_base: (i32, i32),
    video_frame_rate: i32,
    audio_time_base: (i32, i32),
    faststart: bool,
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

        // Check global header flag before adding streams
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

            let codec = ffmpeg::encoder::find(codec_id)
                .context("Failed to find video codec for muxer")?;

            let mut stream = format_context.add_stream(codec)?;
            let stream_index = stream.index();

            // Set basic stream parameters
            stream.set_time_base(video_time_base);
            stream.set_avg_frame_rate((rounded_fps, 1));

            // Use a minimal codec context for basic parameters
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
            faststart: config.faststart,
            expect_audio: config.expect_audio,
        })
    }

    pub fn write_packets(
        &mut self,
        video_packets: &[EncodedPacket],
        audio_packets: &[EncodedPacket],
    ) -> Result<()> {
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

        if self.faststart {
            let mut options = ffmpeg::Dictionary::new();
            options.set("movflags", "+faststart");
            self.format_context
                .write_header_with(options)
                .context("Failed to write MP4 header with faststart")?;
        } else {
            self.format_context
                .write_header()
                .context("Failed to write MP4 header")?;
        }

        let video_count = self.write_video_packets(video_packets, base_qpc)?;
        let audio_count = if let (Some(audio_stream_index), Some(audio_encoder)) =
            (self.audio_stream_index, self.audio_encoder.as_mut())
        {
            let mixed_pcm = mix_audio_packets_to_pcm(audio_packets, base_qpc, video_end_qpc);
            if mixed_pcm.is_empty() && !self.expect_audio {
                0
            } else {
                encode_audio_track(
                    &mut self.format_context,
                    audio_encoder,
                    audio_stream_index,
                    self.audio_time_base,
                    &mixed_pcm,
                )?
            }
        } else {
            0
        };

        self.format_context.write_trailer()?;
        info!(
            "Muxed {} video packets and {} audio packets to {:?}",
            video_count, audio_count, self.output_path
        );

        Ok(())
    }

    fn write_video_packets(
        &mut self,
        video_packets: &[EncodedPacket],
        base_qpc: i64,
    ) -> Result<usize> {
        if video_packets.is_empty() {
            return Ok(0);
        }

        for (index, packet) in video_packets.iter().enumerate() {
            let pts = qpc_to_time_base(packet.pts.saturating_sub(base_qpc), self.video_time_base.1 as i64);
            let dts = qpc_to_time_base(packet.dts.saturating_sub(base_qpc), self.video_time_base.1 as i64);
            let next_pts = video_packets
                .get(index + 1)
                .map(|next| next.pts)
                .unwrap_or_else(|| packet.pts.saturating_add(default_video_frame_qpc(self.video_frame_rate)));
            let duration_qpc = next_pts.saturating_sub(packet.pts).max(0);
            let duration = qpc_to_time_base(duration_qpc, self.video_time_base.1 as i64).max(1);

            write_borrowed_video_packet(
                &mut self.format_context,
                self.video_stream_index,
                packet,
                pts.max(0),
                dts.max(0),
                duration,
            )?;
        }

        Ok(video_packets.len())
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

fn default_video_frame_qpc(video_frame_rate: i32) -> i64 {
    let qpc_freq = crate::buffer::ring::qpc_frequency();
    (qpc_freq / video_frame_rate.max(1) as i64).max(1)
}

fn estimate_video_end_qpc(video_packets: &[EncodedPacket], video_frame_rate: i32, video_tb_den: i32) -> i64 {
    video_packets
        .last()
        .map(|packet| {
            let frame_qpc = default_video_frame_qpc(video_frame_rate).max(
                qpc_to_qpc(1, video_tb_den as i64).max(1),
            );
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
    audio_packets: &[EncodedPacket],
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

fn encode_audio_track(
    format_context: &mut ffmpeg::format::context::Output,
    encoder: &mut ffmpeg::encoder::Audio,
    stream_index: usize,
    stream_time_base: (i32, i32),
    pcm_samples: &[i16],
) -> Result<usize> {
    let input_format = ffmpeg::format::Sample::I16(ffmpeg::format::sample::Type::Packed);
    let channel_layout = ffmpeg::channel_layout::ChannelLayout::STEREO;
    let frame_size = encoder.frame_size().max(1024) as usize;
    let samples_per_chunk = frame_size.saturating_mul(AUDIO_CHANNELS as usize);
    let mut encoded_packets = 0usize;
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
        encoded_packets += drain_audio_packets(format_context, encoder, stream_index, stream_time_base)?;
    }

    encoder.send_eof().ok();
    encoded_packets += drain_audio_packets(format_context, encoder, stream_index, stream_time_base)?;
    Ok(encoded_packets)
}

fn copy_pcm_into_frame(frame: &mut ffmpeg::frame::Audio, chunk: &[i16]) {
    match frame.format() {
        ffmpeg::format::Sample::I16(ffmpeg::format::sample::Type::Packed) => {
            let plane = frame.plane_mut::<(i16, i16)>(0);
            for (dst, src) in plane.iter_mut().zip(chunk.chunks_exact(AUDIO_CHANNELS as usize)) {
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
            for (dst, src) in plane.iter_mut().zip(chunk.chunks_exact(AUDIO_CHANNELS as usize)) {
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
            dst[start..start + std::mem::size_of::<f32>()]
                .copy_from_slice(&sample.to_ne_bytes());
        }
    }
}

fn drain_audio_packets(
    format_context: &mut ffmpeg::format::context::Output,
    encoder: &mut ffmpeg::encoder::Audio,
    stream_index: usize,
    stream_time_base: (i32, i32),
) -> Result<usize> {
    let mut encoded_packets = 0usize;
    let mut packet = ffmpeg::Packet::empty();
    while encoder.receive_packet(&mut packet).is_ok() {
        packet.set_stream(stream_index);
        packet.rescale_ts((1, AUDIO_SAMPLE_RATE as i32), stream_time_base);
        packet.write_interleaved(format_context)?;
        encoded_packets += 1;
        packet = ffmpeg::Packet::empty();
    }

    Ok(encoded_packets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode::StreamType;

    #[test]
    fn mixes_system_and_mic_pcm_into_single_timeline() {
        let base_qpc = 1_000_000;
        let system = EncodedPacket::new(
            vec![1u8, 0, 2, 0, 3, 0, 4, 0],
            base_qpc,
            base_qpc,
            false,
            StreamType::SystemAudio,
        );
        let mic = EncodedPacket::new(
            vec![5u8, 0, 6, 0, 7, 0, 8, 0],
            base_qpc,
            base_qpc,
            false,
            StreamType::Microphone,
        );

        let mixed = mix_audio_packets_to_pcm(&[system, mic], base_qpc, base_qpc);

        assert_eq!(mixed, vec![6, 8, 10, 12]);
    }
}