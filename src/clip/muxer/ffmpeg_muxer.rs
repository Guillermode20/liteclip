use anyhow::{Context, Result};
use ffmpeg_next as ffmpeg;
use std::path::{Path, PathBuf};
use crate::encode::{EncodedPacket};
use tracing::{info};
use ffmpeg::Rescale;

pub struct FfmpegMuxer {
    format_context: ffmpeg::format::context::Output,
    video_stream_index: Option<usize>,
    _audio_stream_index: Option<usize>,
    output_path: PathBuf,
}

impl FfmpegMuxer {
    pub fn new(output_path: &Path, video_codec: &str, width: u32, height: u32, _fps: f64) -> Result<Self> {
        let mut format_context = ffmpeg::format::output(&output_path)
            .context("Failed to create output format context")?;

        let v_idx;

        // 1. Setup Video Stream
        {
            let codec_id = match video_codec {
                "hevc" => ffmpeg::codec::Id::HEVC,
                "av1" => ffmpeg::codec::Id::AV1,
                _ => ffmpeg::codec::Id::H264,
            };

            let codec = ffmpeg::encoder::find(codec_id)
                .context("Failed to find video codec for muxer")?;

            let mut stream = format_context.add_stream(codec)?;
            v_idx = stream.index();
            
            let mut video = ffmpeg::codec::context::Context::new_with_codec(codec).encoder().video()?;
            video.set_width(width);
            video.set_height(height);
            video.set_format(ffmpeg::format::Pixel::YUV420P);
            video.set_time_base((1, 90000));
            
            stream.set_parameters(video);
        }

        info!("Created native muxer for {:?}", output_path);

        Ok(Self {
            format_context,
            video_stream_index: Some(v_idx),
            _audio_stream_index: None,
            output_path: output_path.to_path_buf(),
        })
    }

    pub fn write_packets(&mut self, video_packets: &[EncodedPacket], _audio_packets: &[EncodedPacket]) -> Result<()> {
        self.format_context.write_header()?;

        let mut video_count = 0;

        if let Some(v_idx) = self.video_stream_index {
            let start_pts = video_packets.first().map(|p| p.pts).unwrap_or(0);
            
            for packet in video_packets {
                let mut ffmpeg_packet = ffmpeg::Packet::copy(&packet.data);
                ffmpeg_packet.set_stream(v_idx);
                
                // Normalize timestamps so the clip starts at 0
                let relative_pts = (packet.pts - start_pts).max(0);
                let relative_dts = (packet.dts - start_pts).max(0);
                
                // Use rescale
                let pts = relative_pts.rescale((1, 10_000_000), (1, 90_000));
                let dts = relative_dts.rescale((1, 10_000_000), (1, 90_000));
                
                ffmpeg_packet.set_pts(Some(pts));
                ffmpeg_packet.set_dts(Some(dts));
                
                if packet.is_keyframe {
                    ffmpeg_packet.set_flags(ffmpeg::codec::packet::flag::Flags::KEY);
                }

                ffmpeg_packet.write_interleaved(&mut self.format_context)?;
                video_count += 1;
            }
        }

        self.format_context.write_trailer()?;
        info!("Muxed {} video packets to {:?}", video_count, self.output_path);
        
        Ok(())
    }
}



