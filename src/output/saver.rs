use crate::buffer::ring::SharedReplayBuffer;
use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use super::{
    calculate_clip_start_pts, generate_output_path, generate_thumbnail, h264_nal_type,
    hevc_nal_type, Muxer, MuxerConfig,
};

pub fn spawn_clip_saver(
    buffer: SharedReplayBuffer,
    duration: Duration,
    output_path: PathBuf,
    config: MuxerConfig,
    save_directory: PathBuf,
) -> JoinHandle<Result<PathBuf>> {
    tokio::task::spawn_blocking(move || {
        info!(
            "Clip saver started: duration={}s, output={:?}",
            duration.as_secs(),
            output_path
        );

        let newest_pts = buffer
            .newest_pts()
            .context("No packets in buffer to save")?;
        let oldest_pts = buffer.oldest_pts();

        let start_pts = calculate_clip_start_pts(newest_pts, duration, oldest_pts);
        debug!(
            "Clip window: {} to {} (duration: {}s)",
            start_pts,
            newest_pts,
            duration.as_secs()
        );

        let mut clip_packets = buffer
            .snapshot_from(start_pts)
            .context("Failed to get packets from buffer")?;

        let video_packets: Vec<_> = clip_packets
            .iter()
            .filter(|p| matches!(p.stream, crate::encode::StreamType::Video))
            .collect();

        if !video_packets.is_empty() {
            let first_vid = video_packets[0];
            let first_20_bytes: Vec<String> = first_vid
                .data
                .iter()
                .take(20)
                .map(|b| format!("{:02x}", b))
                .collect();
            info!(
                "First video packet: {}B, keyframe={}, first20=[{}]",
                first_vid.data.len(),
                first_vid.is_keyframe,
                first_20_bytes.join(" ")
            );

            let nal_type = hevc_nal_type(first_vid.data.as_ref());
            info!("First video packet HEVC NAL type: {:?}", nal_type);
        }

        let has_decodable_video_frame = |packets: &[crate::encode::EncodedPacket]| {
            packets.iter().any(|packet| {
                if !matches!(packet.stream, crate::encode::StreamType::Video) {
                    return false;
                }
                if matches!(h264_nal_type(packet.data.as_ref()), Some(1 | 5 | 7 | 8)) {
                    return true;
                }
                if matches!(
                    hevc_nal_type(packet.data.as_ref()),
                    Some(19 | 20 | 32 | 33 | 34)
                ) {
                    return true;
                }
                false
            })
        };

        if !has_decodable_video_frame(&clip_packets) {
            warn!("Clip snapshot does not yet contain a decodable video frame; retrying briefly");
            for attempt in 1..=5 {
                thread::sleep(Duration::from_millis(150));
                clip_packets = buffer
                    .snapshot_from(start_pts)
                    .context("Failed to refresh packets from buffer")?;
                if has_decodable_video_frame(&clip_packets) {
                    info!(
                        "Found decodable video frame after clip snapshot retry {}/5",
                        attempt
                    );
                    break;
                }
            }
        }

        debug!(
            "Clip packets: {} (seeked to nearest keyframe)",
            clip_packets.len()
        );

        let keyframe_count = clip_packets.iter().filter(|p| p.is_keyframe).count();
        if keyframe_count == 0 {
            warn!("No keyframes in clip range - video may not be playable");
        }

        let mut muxer = Muxer::new(&output_path, &config).context("Failed to create muxer")?;

        let mut video_count = 0;
        let mut audio_count = 0;

        for packet in &clip_packets {
            match packet.stream {
                crate::encode::StreamType::Video => {
                    muxer
                        .write_video_packet(packet)
                        .context("Failed to write video packet")?;
                    video_count += 1;
                }
                crate::encode::StreamType::SystemAudio | crate::encode::StreamType::Microphone => {
                    muxer
                        .write_audio_packet(packet)
                        .context("Failed to write audio packet")?;
                    audio_count += 1;
                }
            }
        }

        info!(
            "Prepared clip packet set: {} video packets, {} audio packets",
            video_count, audio_count
        );

        if video_count == 0 {
            bail!("No video packets in selected clip range");
        }

        let final_path = muxer.finalize().context("Failed to finalize MP4")?;

        info!(
            "Clip saved successfully: {:?} ({} video packets, {} audio packets, ~{} seconds)",
            final_path,
            video_count,
            audio_count,
            duration.as_secs()
        );

        // Generate thumbnail immediately after saving
        debug!("Generating thumbnail for saved clip");
        match generate_thumbnail(&final_path, &save_directory) {
            Ok(thumb_path) => {
                info!("Thumbnail generated: {:?}", thumb_path);
            }
            Err(e) => {
                warn!("Failed to generate thumbnail: {}", e);
            }
        }

        Ok(final_path)
    })
}

pub fn spawn_clip_saver_with_defaults(
    buffer: SharedReplayBuffer,
    duration: Duration,
    save_directory: PathBuf,
    game_name: Option<String>,
    width: u32,
    height: u32,
    fps: f64,
) -> Result<JoinHandle<Result<PathBuf>>> {
    let output_path = generate_output_path(&save_directory, game_name.as_deref())
        .context("Failed to generate clip output path")?;

    let config = MuxerConfig::new(width, height, fps, &output_path);

    Ok(spawn_clip_saver(
        buffer,
        duration,
        output_path,
        config,
        save_directory,
    ))
}
