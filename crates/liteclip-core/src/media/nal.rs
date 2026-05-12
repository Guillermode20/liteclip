//! Shared NAL unit type detection for H.264 and HEVC byte streams.
//!
//! These functions are used by both the ring buffer (parameter-set caching during capture)
//! and the output muxer (codec detection, parameter-set merging). Centralizing them here
//! avoids duplicating the NAL-scanning logic across modules.

/// Extracts the H.264 NAL unit type from byte data.
///
/// Supports both start-code (`00 00 01` / `00 00 00 01`) and length-prefixed
/// (4-byte big-endian size) framing.
///
/// # Returns
///
/// NAL unit type (0–31), or `None` if the data is too short or has no recognized framing.
pub fn h264_nal_type(data: &[u8]) -> Option<u8> {
    if data.len() >= 5 && data[0..4] == [0x00, 0x00, 0x00, 0x01] {
        return Some(data[4] & 0x1f);
    }
    if data.len() >= 4 && data[0..3] == [0x00, 0x00, 0x01] {
        return Some(data[3] & 0x1f);
    }
    // Length-prefixed (4-byte big-endian size before NAL header).
    if data.len() >= 5 {
        let nal_len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
        if nal_len > 0 && data.len() >= 4 + nal_len {
            return Some(data[4] & 0x1f);
        }
    }
    None
}

/// Extracts the HEVC (H.265) NAL unit type from byte data.
///
/// Supports both start-code and length-prefixed framing.
///
/// # Returns
///
/// NAL unit type (0–63), or `None` if the data is too short or has no recognized framing.
pub fn hevc_nal_type(data: &[u8]) -> Option<u8> {
    if data.len() >= 6 && data[0..4] == [0x00, 0x00, 0x00, 0x01] {
        return Some((data[4] >> 1) & 0x3f);
    }
    if data.len() >= 5 && data[0..3] == [0x00, 0x00, 0x01] {
        return Some((data[3] >> 1) & 0x3f);
    }
    // Length-prefixed (4-byte big-endian size before 2-byte NAL header).
    if data.len() >= 6 {
        let nal_len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
        if nal_len > 1 && data.len() >= 4 + nal_len {
            return Some((data[4] >> 1) & 0x3f);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- H.264 ---

    #[test]
    fn h264_start_code_4byte() {
        let data = [0x00, 0x00, 0x00, 0x01, 0x67];
        assert_eq!(h264_nal_type(&data), Some(0x67 & 0x1f));
    }

    #[test]
    fn h264_start_code_3byte() {
        let data = [0x00, 0x00, 0x01, 0x65];
        assert_eq!(h264_nal_type(&data), Some(0x65 & 0x1f));
    }

    #[test]
    fn h264_length_prefixed() {
        let data = vec![0x00, 0x00, 0x00, 0x02, 0x21, 0x00];
        assert_eq!(h264_nal_type(&data), Some(0x21 & 0x1f));
    }

    #[test]
    fn h264_too_short() {
        let data = [0x00, 0x00, 0x01];
        assert_eq!(h264_nal_type(&data), None);
    }

    #[test]
    fn h264_sps() {
        let data = [0x00, 0x00, 0x00, 0x01, 0x67];
        assert_eq!(h264_nal_type(&data), Some(7));
    }

    #[test]
    fn h264_idr() {
        let data = [0x00, 0x00, 0x00, 0x01, 0x65];
        assert_eq!(h264_nal_type(&data), Some(5));
    }

    // --- H.265 / HEVC ---

    #[test]
    fn hevc_start_code_4byte() {
        let data = [0x00, 0x00, 0x00, 0x01, 0x40, 0x01];
        assert_eq!(hevc_nal_type(&data), Some((0x40 >> 1) & 0x3f));
    }

    #[test]
    fn hevc_start_code_3byte() {
        let data = [0x00, 0x00, 0x01, 0x42, 0x01];
        assert_eq!(hevc_nal_type(&data), Some((0x42 >> 1) & 0x3f));
    }

    #[test]
    fn hevc_vps() {
        let data = [0x00, 0x00, 0x00, 0x01, 0x40, 0x01];
        assert_eq!(hevc_nal_type(&data), Some(32));
    }

    #[test]
    fn hevc_sps() {
        let data = [0x00, 0x00, 0x00, 0x01, 0x42, 0x01];
        assert_eq!(hevc_nal_type(&data), Some(33));
    }

    #[test]
    fn hevc_idr() {
        let data = [0x00, 0x00, 0x00, 0x01, 0x26, 0x01];
        assert_eq!(hevc_nal_type(&data), Some(19));
    }

    #[test]
    fn hevc_too_short() {
        let data = [0x00, 0x00, 0x01];
        assert_eq!(hevc_nal_type(&data), None);
    }
}
