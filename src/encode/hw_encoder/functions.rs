//! Auto-generated module
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use anyhow::{Context, Result};
use memchr::memmem;
use std::os::windows::process::CommandExt;
use std::process::Command;
use tracing::{debug, info, warn};

const CREATE_NO_WINDOW: u32 = 0x08000000;
/// Process creation flags to prevent console window from appearing.
/// Note: CREATE_NO_WINDOW is ignored if combined with DETACHED_PROCESS,
/// so we use only CREATE_NO_WINDOW to ensure the flag takes effect.
pub const PROCESS_CREATION_FLAGS: u32 = CREATE_NO_WINDOW;

/// Find H.264 Annex B start code (00 00 01 or 00 00 00 01) using SIMD-accelerated search.
///
/// Uses `memchr` to efficiently find candidate positions with `0x00` bytes,
/// then checks for the full start code pattern at those positions.
///
/// # Returns
/// * `Some((position, length))` - position is the start code offset, length is 3 or 4
/// * `None` - if no start code is found
pub(super) fn find_annexb_start_code(data: &[u8], from: usize) -> Option<(usize, usize)> {
    if data.len() < 3 || from >= data.len() {
        return None;
    }

    let search_data = &data[from..];
    if let Some(pos) = memmem::find(search_data, b"\x00\x00\x01") {
        let abs_pos = from + pos;
        if abs_pos > from && data[abs_pos - 1] == 0x00 {
            Some((abs_pos - 1, 4))
        } else {
            Some((abs_pos, 3))
        }
    } else {
        None
    }
}
pub(super) fn h264_nal_type(nal_data: &[u8]) -> Option<u8> {
    if nal_data.len() >= 5
        && nal_data[0] == 0x00
        && nal_data[1] == 0x00
        && nal_data[2] == 0x00
        && nal_data[3] == 0x01
    {
        return Some(nal_data[4] & 0x1f);
    }
    if nal_data.len() >= 4 && nal_data[0] == 0x00 && nal_data[1] == 0x00 && nal_data[2] == 0x01 {
        return Some(nal_data[3] & 0x1f);
    }
    None
}

pub(super) fn hevc_nal_type(nal_data: &[u8]) -> Option<u8> {
    if nal_data.len() >= 6
        && nal_data[0] == 0x00
        && nal_data[1] == 0x00
        && nal_data[2] == 0x00
        && nal_data[3] == 0x01
    {
        return Some((nal_data[4] >> 1) & 0x3f);
    }
    if nal_data.len() >= 5 && nal_data[0] == 0x00 && nal_data[1] == 0x00 && nal_data[2] == 0x01 {
        return Some((nal_data[3] >> 1) & 0x3f);
    }
    None
}

struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }

    fn read_bit(&mut self) -> Option<u8> {
        let byte_index = self.bit_pos / 8;
        let bit_index = 7 - (self.bit_pos % 8);
        let byte = *self.data.get(byte_index)?;
        self.bit_pos += 1;
        Some((byte >> bit_index) & 1)
    }

    fn read_ue(&mut self) -> Option<u32> {
        let mut leading_zeros = 0usize;
        while self.read_bit()? == 0 {
            leading_zeros += 1;
            if leading_zeros > 31 {
                return None;
            }
        }

        let mut suffix = 0u32;
        for _ in 0..leading_zeros {
            suffix = (suffix << 1) | (self.read_bit()? as u32);
        }

        Some((1u32 << leading_zeros) - 1 + suffix)
    }
}

/// Returns true if an H.264 Annex-B NAL appears to be an intra-coded non-IDR slice.
///
/// This is a pragmatic fallback for encoders that emit GOP boundary keyframes as
/// non-IDR NAL type 1 slices instead of NAL type 5 IDR slices.
pub(super) fn h264_nonidr_is_intra_slice(nal_data: &[u8]) -> bool {
    let (start_len, header_index) = if nal_data.len() >= 5 && nal_data[0..4] == [0, 0, 0, 1] {
        (4usize, 4usize)
    } else if nal_data.len() >= 4 && nal_data[0..3] == [0, 0, 1] {
        (3usize, 3usize)
    } else {
        return false;
    };

    let nal_header = match nal_data.get(header_index) {
        Some(v) => *v,
        None => return false,
    };
    let nal_type = nal_header & 0x1f;
    if nal_type != 1 {
        return false;
    }

    if nal_data.len() <= start_len + 1 {
        return false;
    }

    let mut rbsp = Vec::with_capacity(nal_data.len() - (start_len + 1));
    let mut zeros_run = 0usize;
    for &b in &nal_data[(start_len + 1)..] {
        if zeros_run >= 2 && b == 0x03 {
            zeros_run = 0;
            continue;
        }
        rbsp.push(b);
        if b == 0 {
            zeros_run += 1;
        } else {
            zeros_run = 0;
        }
    }

    if rbsp.is_empty() {
        return false;
    }

    let mut br = BitReader::new(&rbsp);
    // first_mb_in_slice
    if br.read_ue().is_none() {
        return false;
    }
    // slice_type
    let Some(slice_type) = br.read_ue() else {
        return false;
    };

    // H.264 slice_type modulo 5 mapping:
    // 0=P,1=B,2=I,3=SP,4=SI ; 5..9 are the "all slices" variants
    let normalized = slice_type % 5;
    normalized == 2 || normalized == 4
}
/// Resolve FFmpeg command path
pub(super) fn resolve_ffmpeg_command() -> String {
    if let Ok(custom) = std::env::var("LITECLIP_FFMPEG_PATH") {
        if !custom.trim().is_empty() {
            return custom;
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        let candidate = cwd.join("ffmpeg").join("bin").join("liteclip-replay-ffmpeg.exe");
        if candidate.exists() {
            return candidate.to_string_lossy().to_string();
        }
    }
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let candidate = exe_dir.join("ffmpeg").join("bin").join("liteclip-replay-ffmpeg.exe");
            if candidate.exists() {
                return candidate.to_string_lossy().to_string();
            }
        }
    }
    "liteclip-replay-ffmpeg".to_string()
}
pub(super) fn query_qpc() -> Result<i64> {
    let mut qpc = 0i64;
    unsafe { windows::Win32::System::Performance::QueryPerformanceCounter(&mut qpc) }
        .context("QueryPerformanceCounter failed")?;
    Ok(qpc)
}
/// Check if a hardware encoder is available by attempting to probe it
#[cfg(feature = "ffmpeg")]
pub fn check_encoder_available(encoder_name: &str) -> bool {
    let ffmpeg_cmd = resolve_ffmpeg_command();
    let output = Command::new(&ffmpeg_cmd)
        .arg("-hide_banner")
        .arg("-encoders")
        .arg("-v")
        .arg("error")
        .creation_flags(PROCESS_CREATION_FLAGS)
        .output();
    let listed = match output {
        Ok(out) => {
            let output_str = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            output_str.contains(encoder_name)
        }
        Err(e) => {
            warn!(
                "Failed to check encoder listing for {}: {}",
                encoder_name, e
            );
            return false;
        }
    };
    if !listed {
        info!("Encoder {} not found in FFmpeg", encoder_name);
        return false;
    }
    let mut probe_cmd = Command::new(&ffmpeg_cmd);
    probe_cmd
        .arg("-hide_banner")
        .arg("-v")
        .arg("error")
        .arg("-f")
        .arg("lavfi")
        .arg("-i")
        .arg("nullsrc=s=320x240:d=0.04")
        .arg("-c:v")
        .arg(encoder_name)
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg("-frames:v")
        .arg("1");
    match encoder_name {
        "h264_amf" | "hevc_amf" | "av1_amf" => {
            probe_cmd.arg("-quality").arg("speed");
            probe_cmd.arg("-bf").arg("0");
        }
        "h264_nvenc" | "hevc_nvenc" | "av1_nvenc" => {
            probe_cmd.arg("-preset").arg("p4");
        }
        "h264_qsv" | "hevc_qsv" => {
            probe_cmd.arg("-preset").arg("veryfast");
        }
        _ => {}
    }
    debug!("Probing encoder {} with FFmpeg", encoder_name);
    let probe = probe_cmd
        .creation_flags(PROCESS_CREATION_FLAGS)
        .arg("-f")
        .arg("null")
        .arg("-")
        .output();
    match probe {
        Ok(out) => {
            if out.status.success() {
                info!("Encoder {} probe succeeded", encoder_name);
                true
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let first_line = stderr
                    .lines()
                    .next()
                    .unwrap_or("unknown ffmpeg probe error")
                    .trim();
                info!(
                    "Encoder {} is listed but probe failed - may indicate missing/broken driver",
                    encoder_name
                );
                warn!("Encoder {} probe failed: {}", encoder_name, first_line);
                debug!("Encoder {} probe stderr:\n{}", encoder_name, stderr.trim());
                false
            }
        }
        Err(e) => {
            warn!("Failed to probe encoder {}: {}", encoder_name, e);
            false
        }
    }
}
#[cfg(not(feature = "ffmpeg"))]
pub fn check_encoder_available(_encoder_name: &str) -> bool {
    false
}
#[cfg(test)]
mod tests {
    use super::super::types::{AmfEncoder, NvencEncoder, QsvEncoder};
    use crate::encode::EncoderConfig;
    fn create_test_config() -> EncoderConfig {
        EncoderConfig::new(
            crate::config::Codec::H264,
            20,
            30,
            (1920, 1080),
            crate::config::EncoderType::Nvenc,
            1,
        )
    }
    #[test]
    fn test_nvenc_encoder_creation() {
        let config = create_test_config();
        let encoder = NvencEncoder::new(&config);
        assert!(encoder.is_ok());
    }
    #[test]
    fn test_amf_encoder_creation() {
        let config = EncoderConfig::new(
            crate::config::Codec::H264,
            20,
            30,
            (1920, 1080),
            crate::config::EncoderType::Amf,
            1,
        );
        let encoder = AmfEncoder::new(&config);
        assert!(encoder.is_ok());
    }
    #[test]
    fn test_qsv_encoder_creation() {
        let config = EncoderConfig::new(
            crate::config::Codec::H264,
            20,
            30,
            (1920, 1080),
            crate::config::EncoderType::Qsv,
            1,
        );
        let encoder = QsvEncoder::new(&config);
        assert!(encoder.is_ok());
    }
}
