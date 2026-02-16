//! Auto-generated module
//!
//! 🤖 Generated with [SplitRS](https://github.com/cool-japan/splitrs)

use anyhow::{Context, Result};
use memchr::memchr;
use std::process::Command;
use tracing::{debug, info, warn};

/// Convert BGRA to RGB24 format for FFmpeg input
#[cfg(test)]
fn bgra_to_rgb24(bgra: &[u8], width: u32, height: u32) -> Vec<u8> {
    let expected_len = (width * height * 4) as usize;
    if bgra.len() != expected_len {
        return vec![];
    }
    let pixel_count = (width * height) as usize;
    let mut rgb = Vec::with_capacity(pixel_count * 3);
    unsafe {
        let mut dst: *mut u8 = rgb.as_mut_ptr();
        let src: *const u8 = bgra.as_ptr();
        for i in 0..pixel_count {
            let src_offset = i * 4;
            dst.write(*src.add(src_offset + 2));
            dst = dst.add(1);
            dst.write(*src.add(src_offset + 1));
            dst = dst.add(1);
            dst.write(*src.add(src_offset));
            dst = dst.add(1);
        }
        rgb.set_len(pixel_count * 3);
    }
    rgb
}
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

    // Start search from 'from', ensuring we have room to check for patterns
    let search_start = from;
    let search_data = &data[search_start..];

    // Use memchr to find 0x00 bytes efficiently with SIMD acceleration
    let mut offset = 0;
    while let Some(pos) = memchr(0x00, &search_data[offset..]) {
        let abs_pos = search_start + offset + pos;

        // Check for 4-byte start code: 00 00 00 01
        if abs_pos + 3 < data.len()
            && data[abs_pos] == 0x00
            && data[abs_pos + 1] == 0x00
            && data[abs_pos + 2] == 0x00
            && data[abs_pos + 3] == 0x01
        {
            return Some((abs_pos, 4));
        }

        // Check for 3-byte start code: 00 00 01
        if abs_pos + 2 < data.len()
            && data[abs_pos] == 0x00
            && data[abs_pos + 1] == 0x00
            && data[abs_pos + 2] == 0x01
        {
            return Some((abs_pos, 3));
        }

        // Move past this 0x00 to find the next candidate
        offset = pos + 1;

        // Prevent infinite loop and ensure we have room for patterns
        if offset + 2 >= search_data.len() {
            break;
        }
    }

    None
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
/// Resolve FFmpeg command path
pub(super) fn resolve_ffmpeg_command() -> String {
    if let Ok(custom) = std::env::var("LITECLIP_FFMPEG_PATH") {
        if !custom.trim().is_empty() {
            return custom;
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        let candidate = cwd.join("ffmpeg").join("bin").join("ffmpeg.exe");
        if candidate.exists() {
            return candidate.to_string_lossy().to_string();
        }
    }
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let candidate = exe_dir.join("ffmpeg").join("bin").join("ffmpeg.exe");
            if candidate.exists() {
                return candidate.to_string_lossy().to_string();
            }
        }
    }
    "ffmpeg".to_string()
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
    let probe = probe_cmd.arg("-f").arg("null").arg("-").output();
    match probe {
        Ok(out) => {
            if out.status.success() {
                info!("Encoder {} probe succeeded", encoder_name);
                true
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr);
                info!(
                    "Encoder {} is listed but probe failed - may indicate missing/broken driver",
                    encoder_name
                );
                warn!("Encoder {} probe failed: {}", encoder_name, stderr.trim());
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
    use super::*;
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
    #[test]
    fn test_bgra_to_rgb24() {
        let bgra: Vec<u8> = vec![
            0x00, 0x00, 0xFF, 0xFF, 0x00, 0xFF, 0x00, 0xFF, 0xFF, 0x00, 0x00, 0xFF,
        ];
        let rgb = bgra_to_rgb24(&bgra, 3, 1);
        assert_eq!(rgb.len(), 9);
        assert_eq!(rgb[0], 0xFF);
        assert_eq!(rgb[1], 0x00);
        assert_eq!(rgb[2], 0x00);
    }
    #[test]
    fn test_quality_preset_maps_to_nvenc_presets() {
        let mut config = create_test_config();
        config.quality_preset = QualityPreset::Performance;
        let base = HardwareEncoderBase::new(&config, "h264_nvenc").expect("base");
        assert_eq!(base.preset_for_encoder("h264_nvenc"), "p3");
        config.quality_preset = QualityPreset::Balanced;
        let base = HardwareEncoderBase::new(&config, "h264_nvenc").expect("base");
        assert_eq!(base.preset_for_encoder("h264_nvenc"), "p5");
        config.quality_preset = QualityPreset::Quality;
        let base = HardwareEncoderBase::new(&config, "h264_nvenc").expect("base");
        assert_eq!(base.preset_for_encoder("h264_nvenc"), "p7");
    }
    #[test]
    fn test_cq_default_value_uses_quality_preset() {
        let mut config = create_test_config();
        config.rate_control = RateControl::Cq;
        config.quality_preset = QualityPreset::Quality;
        config.quality_value = None;
        let base = HardwareEncoderBase::new(&config, "h264_nvenc").expect("base");
        assert_eq!(base.cq_value(), 19);
    }
}
