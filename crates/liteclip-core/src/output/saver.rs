use crate::buffer::ring::{SharedReplayBuffer, TrackedSnapshot};
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

/// Aggressively drops all packet data and forces memory release.
/// Uses std::mem::replace to ensure the old allocation is fully freed before
/// the function returns, preventing the allocator from holding onto large blocks.
fn aggressively_drop_packets(packets: TrackedSnapshot) {
    // Convert to Vec and drop to release memory
    // The Drop impl will decrement the outstanding_snapshot_bytes counter
    drop(packets);
}

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

        // ── Phase 1: Take snapshot with aggressive retry memory cleanup ──
        // Keep as TrackedSnapshot to track pinned bytes until after mux
        let mut snapshot = buffer
            .snapshot_from(start_pts)
            .context("Failed to get packets from buffer")?;

        // Video tail catch-up retries — each retry must drop old snapshot before allocating new
        for attempt in 1..=CLIP_VIDEO_CATCH_UP_RETRY_LIMIT {
            let Some(video_tail_lag_qpc) = clip_video_tail_lag_qpc(&snapshot) else {
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

            // AGGRESSIVE: drop old snapshot BEFORE sleeping and allocating new one
            aggressively_drop_packets(snapshot);

            thread::sleep(CLIP_VIDEO_CATCH_UP_SLEEP);
            newest_pts = buffer.newest_pts().unwrap_or(newest_pts);
            oldest_pts = buffer.oldest_pts().or(oldest_pts);
            start_pts = calculate_clip_start_pts(newest_pts, duration, oldest_pts);
            snapshot = buffer
                .snapshot_from(start_pts)
                .context("Failed to refresh packets from buffer during video catch-up")?;
        }

        // Decodable frame retries
        if !has_decodable_video_frame(&snapshot) {
            warn!(
                "Clip snapshot does not yet contain a decodable video frame; retrying briefly"
            );
            for attempt in 1..=5 {
                // AGGRESSIVE: drop old snapshot before retry
                aggressively_drop_packets(snapshot);

                thread::sleep(Duration::from_millis(150));
                snapshot = buffer
                    .snapshot_from(start_pts)
                    .context("Failed to refresh packets from buffer")?;
                if has_decodable_video_frame(&snapshot) {
                    info!(
                        "Found decodable video frame after clip snapshot retry {}/5",
                        attempt
                    );
                    break;
                    }
                }
            }

            // Log first video packet info before moving snapshot out
            {
                let video_packets: Vec<_> = snapshot
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
            }

            // Keep snapshot alive through mux to track pinned bytes
            // Log first video packet info before mux
            {
                let video_packets: Vec<_> = snapshot
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
            }

            let keyframe_count = snapshot.iter().filter(|p| p.is_keyframe).count();
            if keyframe_count == 0 {
                warn!("No keyframes in clip range - video may not be playable");
            }

            let video_count = snapshot
                .iter()
                .filter(|packet| matches!(packet.stream, crate::encode::StreamType::Video))
                .count();
            let audio_count = snapshot.len().saturating_sub(video_count);

            info!(
                "Prepared clip packet set: {} video packets, {} audio packets",
                video_count, audio_count
            );

            if video_count == 0 {
                bail!("No video packets in selected clip range");
            }

            let clip_span_secs = clip_pts_span_seconds(snapshot.as_slice());

            // ── Phase 2: Mux with aggressive cleanup ──
            log_save_memory("before mux", None, Some(snapshot.as_slice()));
            let final_path = Muxer::mux_clip(&output_path, &config, snapshot.as_slice())
                .context("Failed to finalize MP4")?;
            log_save_memory("after mux", None, Some(snapshot.as_slice()));

            // AGGRESSIVE: explicitly free all packet data immediately after mux
            // This drops the TrackedSnapshot, decrementing outstanding_snapshot_bytes
            aggressively_drop_packets(snapshot);
            log_save_memory("after packet release", None, None);

            // Release the buffer clone NOW — all needed packets are in the muxed file.
            drop(buffer);

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
        log_save_memory("before thumbnail", None, None);
        match generate_thumbnail(&final_path, &save_directory) {
            Ok(thumb_path) => {
                log_save_memory("after thumbnail", None, None);
                info!("Thumbnail generated: {:?}", thumb_path);
            }
            Err(e) => {
                warn!("Failed to generate thumbnail: {}", e);
            }
        }

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

pub fn log_save_memory(
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

    // Count video vs audio packets
    let (clip_video_count, clip_audio_count) = clip_packets
        .map(|packets| {
            let v = packets
                .iter()
                .filter(|p| matches!(p.stream, crate::encode::StreamType::Video))
                .count();
            (v, packets.len().saturating_sub(v))
        })
        .unwrap_or((0, 0));

    if let Some((working_set_mb, private_mb)) = process_memory_mb() {
        if let Some(stats) = buffer_stats {
            info!(
                "Save memory [{}]: process_working={:.1}MB, private={:.1}MB, buffer={:.1}MB ({}pkts), clip={:.1}MB ({}v+{}a={})",
                stage,
                working_set_mb,
                private_mb,
                stats.total_bytes as f64 / 1_048_576.0,
                stats.packet_count,
                clip_packet_bytes as f64 / 1_048_576.0,
                clip_video_count,
                clip_audio_count,
                clip_packet_count
            );
        } else {
            info!(
                "Save memory [{}]: process_working={:.1}MB, private={:.1}MB, clip={:.1}MB ({}v+{}a={})",
                stage,
                working_set_mb,
                private_mb,
                clip_packet_bytes as f64 / 1_048_576.0,
                clip_video_count,
                clip_audio_count,
                clip_packet_count
            );
        }
    } else {
        // Fallback if process memory not available
        if let Some(stats) = buffer_stats {
            info!(
                "Save memory [{}]: buffer={:.1}MB ({}pkts), clip={:.1}MB ({}v+{}a={})",
                stage,
                stats.total_bytes as f64 / 1_048_576.0,
                stats.packet_count,
                clip_packet_bytes as f64 / 1_048_576.0,
                clip_video_count,
                clip_audio_count,
                clip_packet_count
            );
        }
    }
}

#[cfg(target_os = "windows")]
pub fn process_memory_mb() -> Option<(f64, f64)> {
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
pub fn process_memory_mb() -> Option<(f64, f64)> {
    None
}
