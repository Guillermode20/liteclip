//! Clip Finaliser
//!
//! On save trigger: snapshot buffer → mux to MP4 → write to disk.
//! Uses FFmpeg via ffmpeg-next for MP4 container muxing.

use crate::buffer::ring::SharedReplayBuffer;
use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

pub mod muxer;

pub use muxer::{extract_thumbnail, generate_output_path, Muxer, MuxerConfig};

/// Spawn a clip saver task using tokio::task::spawn_blocking
///
/// This function:
/// 1. Acquires read lock on buffer
/// 2. Snapshots packets from `now - duration` to now
/// 3. Seeks to nearest keyframe
/// 4. Creates muxer and writes packets to MP4
/// 5. Finalizes and returns output path
///
/// Runs in a blocking task to avoid blocking the async runtime during I/O.
pub fn spawn_clip_saver(
    buffer: SharedReplayBuffer,
    duration: Duration,
    output_path: PathBuf,
    config: MuxerConfig,
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

        // Step 2: Find time window and seek to nearest keyframe
        let start_pts = muxer::calculate_clip_start_pts(newest_pts, duration);
        debug!(
            "Clip window: {} to {} (duration: {}s)",
            start_pts,
            newest_pts,
            duration.as_secs()
        );

        // Step 3: Get packets from keyframe
        let mut clip_packets = buffer
            .snapshot_from(start_pts)
            .context("Failed to get packets from buffer")?;

        // Check for decodable video frames (supports both H.264 and HEVC)
        let has_decodable_video_frame = |packets: &[crate::encode::EncodedPacket]| {
            packets.iter().any(|packet| {
                if !matches!(packet.stream, crate::encode::StreamType::Video) {
                    return false;
                }
                // Check H.264 NAL types: 1 (slice), 5 (IDR), 7 (SPS), 8 (PPS)
                if matches!(
                    muxer::h264_nal_type(packet.data.as_ref()),
                    Some(1 | 5 | 7 | 8)
                ) {
                    return true;
                }
                // Check HEVC NAL types: 19/20 (IDR), 32 (VPS), 33 (SPS), 34 (PPS)
                if matches!(
                    muxer::hevc_nal_type(packet.data.as_ref()),
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

        // Verify we have at least one keyframe
        let keyframe_count = clip_packets.iter().filter(|p| p.is_keyframe).count();
        if keyframe_count == 0 {
            warn!("No keyframes in clip range - video may not be playable");
        }

        // Step 4: Create muxer and write packets
        let mut muxer = Muxer::new(&output_path, &config).context("Failed to create muxer")?;

        // Step 5: Write video and audio packets
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

        // Step 6: Finalize the MP4 file
        let final_path = muxer.finalize().context("Failed to finalize MP4")?;

        info!(
            "Clip saved successfully: {:?} ({} video packets, {} audio packets, ~{} seconds)",
            final_path,
            video_count,
            audio_count,
            duration.as_secs()
        );

        // Step 7: Optional thumbnail extraction (Phase 1 optional)
        // Find first keyframe for thumbnail
        if let Some(first_keyframe) = clip_packets.iter().find(|p| p.is_keyframe) {
            debug!("Extracting thumbnail from first keyframe");
            match extract_thumbnail(first_keyframe, &final_path) {
                Ok(thumb_path) => {
                    debug!("Thumbnail extracted: {:?}", thumb_path);
                }
                Err(e) => {
                    warn!("Failed to extract thumbnail: {}", e);
                    // Don't fail the save if thumbnail extraction fails
                }
            }
        }

        Ok(final_path)
    })
}

/// Spawn clip saver with default configuration
///
/// Convenience function that generates output path and uses default muxer config.
pub fn spawn_clip_saver_with_defaults(
    buffer: SharedReplayBuffer,
    duration: Duration,
    save_directory: PathBuf,
    width: u32,
    height: u32,
    fps: f64,
) -> Result<JoinHandle<Result<PathBuf>>> {
    let output_path = muxer::generate_output_path(&save_directory)
        .context("Failed to generate clip output path")?;

    let config = MuxerConfig::new(width, height, fps, &output_path);

    Ok(spawn_clip_saver(buffer, duration, output_path, config))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_muxer_config() {
        let config = MuxerConfig::new(1920, 1080, 30.0, "/tmp/test.mp4");
        assert_eq!(config.width, 1920);
        assert_eq!(config.height, 1080);
        assert_eq!(config.fps, 30.0);
    }
}
