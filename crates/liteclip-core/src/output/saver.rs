use crate::buffer::ring::SharedReplayBuffer;
use crate::encode::EncodedPacket;
use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use super::{
    calculate_clip_start_pts, generate_thumbnail, h264_nal_type, hevc_nal_type, Muxer, MuxerConfig,
};

const CLIP_VIDEO_CATCH_UP_RETRY_LIMIT: usize = 8;
const CLIP_VIDEO_CATCH_UP_SLEEP: Duration = Duration::from_millis(125);

/// Spawns a background task to extract packets from the replay buffer and save them to an MP4 file.
///
/// This function coordinates the following:
/// 1. Snapshotting: Atomsically clones the relevant range of packets from the lock-free `SharedReplayBuffer`.
/// 2. Keyframe Seeking: Ensures the clip starts on a decodable keyframe (IDR frame) to avoid green/corrupt frames.
/// 3. Muxing: Uses `FfmpegMuxer` to interleave video and audio streams into a valid MP4 container.
/// 4. Thumbnail Generation: Spawns a side task to create a JPG preview for the gallery.
///
/// # Arguments
///
/// * `buffer` - The ring buffer containing encoded packets.
/// * `duration` - Requested duration of the clip in seconds.
/// * `output_path` - Target file path for the MP4.
/// * `config` - Muxing parameters (bitrate, flags).
/// * `save_directory` - Root directory for clips (used for thumbnail placement).
///
/// # Returns
///
/// A `JoinHandle` representing the background operation. It resolves to the `PathBuf` of the saved file.
pub fn spawn_clip_saver(
    buffer: SharedReplayBuffer,
    duration: Duration,
    output_path: PathBuf,
    config: MuxerConfig,
    save_directory: PathBuf,
) -> JoinHandle<Result<PathBuf>> {
    tokio::task::spawn_blocking(move || {
        log_save_memory("start", Some(&buffer), None);
        info!(
            "Clip saver started: duration={}s, output={:?}",
            duration.as_secs(),
            output_path
        );

        let mut newest_pts = buffer
            .newest_pts()
            .context("No packets in buffer to save")?;
        let mut oldest_pts = buffer.oldest_pts();

        let mut start_pts = calculate_clip_start_pts(newest_pts, duration, oldest_pts);

        debug!(
            "Clip window: {} to {} (duration: {}s)",
            start_pts,
            newest_pts,
            duration.as_secs()
        );

        let mut clip_packets = buffer
            .snapshot_from(start_pts)
            .context("Failed to get packets from buffer")?;
        log_save_memory("after snapshot", Some(&buffer), Some(&clip_packets));

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

        let max_video_tail_lag_qpc = (crate::buffer::ring::qpc_frequency().max(1) / 2).max(1);

        for attempt in 1..=CLIP_VIDEO_CATCH_UP_RETRY_LIMIT {
            let Some(video_tail_lag_qpc) = clip_video_tail_lag_qpc(&clip_packets) else {
                break;
            };

            if video_tail_lag_qpc <= max_video_tail_lag_qpc {
                break;
            }

            warn!(
                "Clip snapshot video tail is behind newest buffered packet by {}ms; retrying catch-up {}/{}",
                video_tail_lag_qpc.saturating_mul(1000) / crate::buffer::ring::qpc_frequency().max(1),
                attempt,
                CLIP_VIDEO_CATCH_UP_RETRY_LIMIT
            );

            thread::sleep(CLIP_VIDEO_CATCH_UP_SLEEP);
            newest_pts = buffer.newest_pts().unwrap_or(newest_pts);
            oldest_pts = buffer.oldest_pts().or(oldest_pts);
            start_pts = calculate_clip_start_pts(newest_pts, duration, oldest_pts);
            clip_packets = buffer
                .snapshot_from(start_pts)
                .context("Failed to refresh packets from buffer during video catch-up")?;
        }

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

        // Release the buffer clone now — all needed packets are in `clip_packets`.
        // This drops the Arc<LockFreeInner> held by the saver task so the ring
        // buffer's eviction path can immediately free old `Bytes` allocations
        // instead of keeping them alive through the mux + thumbnail phases.
        drop(buffer);

        debug!(
            "Clip packets: {} (seeked to nearest keyframe)",
            clip_packets.len()
        );

        let keyframe_count = clip_packets.iter().filter(|p| p.is_keyframe).count();
        if keyframe_count == 0 {
            warn!("No keyframes in clip range - video may not be playable");
        }

        let video_count = clip_packets
            .iter()
            .filter(|packet| matches!(packet.stream, crate::encode::StreamType::Video))
            .count();
        let audio_count = clip_packets.len().saturating_sub(video_count);

        info!(
            "Prepared clip packet set: {} video packets, {} audio packets",
            video_count, audio_count
        );

        if video_count == 0 {
            bail!("No video packets in selected clip range");
        }

        let clip_span_secs = clip_pts_span_seconds(&clip_packets);

        let final_path = Muxer::mux_clip(&output_path, &config, &clip_packets)
            .context("Failed to finalize MP4")?;
        log_save_memory("after mux", None, Some(&clip_packets));
        drop(clip_packets);
        log_save_memory("after packet release", None, None);

        info!(
            "Clip saved successfully: {:?} ({} video packets, {} audio packets, ~{:.1}s)",
            final_path,
            video_count,
            audio_count,
            clip_span_secs.unwrap_or_else(|| duration.as_secs_f64())
        );

        // Clean up any leftover fragmented MP4s from prior failed saves.
        if let Some(stem) = final_path.file_stem().and_then(|s| s.to_str()) {
            let fragmented_path = final_path
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join(format!("{}.fragmented.mp4", stem));
            if fragmented_path.exists() {
                if let Err(err) = std::fs::remove_file(&fragmented_path) {
                    warn!(
                        "Failed to remove stale fragmented MP4 {:?}: {}",
                        fragmented_path, err
                    );
                }
            }
        }

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
        log_save_memory("after thumbnail", None, None);

        Ok(final_path)
    })
}

/// Presentation timestamps use QPC ticks at ~10 MHz (`EncodedPacket::pts`).
const QPC_TICKS_PER_SEC: f64 = 10_000_000.0;

fn clip_video_tail_lag_qpc(packets: &[EncodedPacket]) -> Option<i64> {
    let newest_packet_pts = packets.iter().map(|p| p.pts).max()?;
    let newest_video_pts = packets
        .iter()
        .filter(|p| matches!(p.stream, crate::encode::StreamType::Video))
        .map(|p| p.pts)
        .max()?;
    Some(newest_packet_pts.saturating_sub(newest_video_pts))
}

fn clip_pts_span_seconds(packets: &[EncodedPacket]) -> Option<f64> {
    if packets.is_empty() {
        return None;
    }
    let min_pts = packets.iter().map(|p| p.pts).min()?;
    let max_pts = packets.iter().map(|p| p.pts).max()?;
    Some(max_pts.saturating_sub(min_pts) as f64 / QPC_TICKS_PER_SEC)
}

fn log_save_memory(
    stage: &str,
    buffer: Option<&SharedReplayBuffer>,
    clip_packets: Option<&[crate::encode::EncodedPacket]>,
) {
    let buffer_stats = buffer.map(SharedReplayBuffer::stats);
    let clip_packet_count = clip_packets.map(|packets| packets.len()).unwrap_or(0);
    let clip_packet_bytes = clip_packets
        .map(|packets| {
            packets
                .iter()
                .map(|packet| packet.data.len())
                .sum::<usize>()
        })
        .unwrap_or(0);

    if let Some((working_set_mb, private_mb)) = process_memory_mb() {
        if let Some(stats) = buffer_stats {
            info!(
                "Save memory [{}]: process_working_set_mb={:.1}, process_private_mb={:.1}, buffer_mb={:.1}, buffer_packets={}, clip_packets={}, clip_packet_mb={:.1}",
                stage,
                working_set_mb,
                private_mb,
                stats.total_bytes as f64 / (1024.0 * 1024.0),
                stats.packet_count,
                clip_packet_count,
                clip_packet_bytes as f64 / (1024.0 * 1024.0)
            );
        } else {
            info!(
                "Save memory [{}]: process_working_set_mb={:.1}, process_private_mb={:.1}, clip_packets={}, clip_packet_mb={:.1}",
                stage,
                working_set_mb,
                private_mb,
                clip_packet_count,
                clip_packet_bytes as f64 / (1024.0 * 1024.0)
            );
        }
    }
}

#[cfg(target_os = "windows")]
fn process_memory_mb() -> Option<(f64, f64)> {
    use std::mem::size_of;
    use windows::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows::Win32::System::Threading::GetCurrentProcess;

    unsafe {
        let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
        if K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters as *mut _ as *mut _,
            size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        )
        .as_bool()
        {
            return Some((
                counters.WorkingSetSize as f64 / (1024.0 * 1024.0),
                counters.PrivateUsage as f64 / (1024.0 * 1024.0),
            ));
        }
    }

    None
}

#[cfg(not(target_os = "windows"))]
fn process_memory_mb() -> Option<(f64, f64)> {
    None
}
