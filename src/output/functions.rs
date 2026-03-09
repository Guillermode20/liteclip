use crate::encode::EncodedPacket;
use anyhow::{Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing::debug;

#[cfg(feature = "ffmpeg")]
pub const AUDIO_SAMPLE_RATE: u32 = 48_000;
#[cfg(feature = "ffmpeg")]
pub const AUDIO_CHANNELS: u16 = 2;
#[cfg(feature = "ffmpeg")]
pub const AUDIO_BITRATE: &str = "192k";

#[cfg(feature = "ffmpeg")]
pub fn qpc_delta_to_aligned_pcm_bytes(
    delta_qpc: i64,
    qpc_freq: f64,
    bytes_per_second: f64,
    bytes_per_frame: usize,
) -> i64 {
    if qpc_freq <= 0.0 || bytes_per_second <= 0.0 || bytes_per_frame == 0 {
        return 0;
    }
    let raw_bytes = ((delta_qpc as f64 / qpc_freq) * bytes_per_second).round() as i64;
    let frame_size = bytes_per_frame as i64;
    if raw_bytes >= 0 {
        raw_bytes - (raw_bytes % frame_size)
    } else {
        raw_bytes + ((-raw_bytes) % frame_size)
    }
}

#[cfg(feature = "ffmpeg")]
pub fn write_silence_bytes(file: &mut std::fs::File, mut byte_count: usize) -> Result<()> {
    if byte_count == 0 {
        return Ok(());
    }
    let silence = [0u8; 8192];
    while byte_count > 0 {
        let chunk = byte_count.min(silence.len());
        file.write_all(&silence[..chunk])
            .context("Failed writing PCM silence padding")?;
        byte_count -= chunk;
    }
    Ok(())
}

pub fn is_h264_format(data: &[u8]) -> bool {
    if data.len() < 4 {
        return false;
    }
    if data.len() >= 4 && data[0] == 0x00 && data[1] == 0x00 && data[2] == 0x00 && data[3] == 0x01 {
        return true;
    }
    if data.len() >= 3 && data[0] == 0x00 && data[1] == 0x00 && data[2] == 0x01 {
        return true;
    }
    matches!(h264_nal_type(data), Some(1..=23))
}

pub fn h264_nal_type(data: &[u8]) -> Option<u8> {
    if data.len() >= 5 && data[0..4] == [0x00, 0x00, 0x00, 0x01] {
        return Some(data[4] & 0x1f);
    }
    if data.len() >= 4 && data[0..3] == [0x00, 0x00, 0x01] {
        return Some(data[3] & 0x1f);
    }
    if data.len() >= 5 {
        let nal_len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
        if nal_len > 0 && data.len() >= 4 + nal_len {
            return Some(data[4] & 0x1f);
        }
    }
    None
}

pub fn hevc_nal_type(data: &[u8]) -> Option<u8> {
    if data.len() >= 6 && data[0..4] == [0x00, 0x00, 0x00, 0x01] {
        return Some((data[4] >> 1) & 0x3f);
    }
    if data.len() >= 5 && data[0..3] == [0x00, 0x00, 0x01] {
        return Some((data[3] >> 1) & 0x3f);
    }
    if data.len() >= 6 {
        let nal_len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
        if nal_len > 1 && data.len() >= 4 + nal_len {
            return Some((data[4] >> 1) & 0x3f);
        }
    }
    None
}

pub fn calculate_clip_start_pts(
    newest_pts: i64,
    duration: std::time::Duration,
    oldest_pts: Option<i64>,
) -> i64 {
    let qpc_freq = crate::buffer::ring::functions::qpc_frequency();
    let duration_qpc = (duration.as_secs_f64() * qpc_freq as f64) as i64;

    let available_duration_qpc = if let Some(oldest) = oldest_pts {
        newest_pts.saturating_sub(oldest)
    } else {
        duration_qpc
    };

    let has_full_duration = available_duration_qpc >= duration_qpc;

    let start_pts = if has_full_duration {
        let skip_qpc = qpc_freq;
        (newest_pts - duration_qpc + skip_qpc).max(skip_qpc)
    } else {
        newest_pts.saturating_sub(available_duration_qpc).max(0)
    };

    let start_pts = start_pts.max(0);

    debug!(
        "Clip window: newest_pts={}, requested_duration={}s, available_duration={}s, has_full={}, start_pts={}",
        newest_pts,
        duration.as_secs(),
        available_duration_qpc / qpc_freq,
        has_full_duration,
        start_pts
    );
    start_pts
}

pub fn generate_output_filename() -> String {
    let timestamp = chrono::Local::now();
    format!("{}.mp4", timestamp.format("%Y-%m-%d_%H-%M-%S_%3f"))
}

pub fn generate_output_path(base_dir: &Path, game_name: Option<&str>) -> Result<PathBuf> {
    let filename = generate_output_filename();

    let output_dir = if let Some(game) = game_name {
        if game.is_empty() {
            base_dir.to_path_buf()
        } else {
            base_dir.join(game)
        }
    } else {
        base_dir.to_path_buf()
    };

    std::fs::create_dir_all(&output_dir)
        .with_context(|| format!("Failed to create output directory: {:?}", output_dir))?;

    Ok(output_dir.join(&filename))
}

pub fn extract_thumbnail(_packet: &EncodedPacket, output_path: &Path) -> Result<PathBuf> {
    debug!("Thumbnail extraction not implemented (optional Phase 1)");
    let thumb_path = output_path.with_extension("jpg");
    Ok(thumb_path)
}
