//! Test fixtures and data generators for integration tests.
//!
//! This module provides factory functions for creating test data including
//! captured frames, encoded packets, and config TOML strings. These utilities
//! ensure consistent test data across the test suite.

use bytes::Bytes;

use liteclip_core::encode::{EncodedPacket, StreamType};
use liteclip_core::media::CapturedFrame;

/// Create a test CapturedFrame with synthetic BGRA data.
///
/// Generates a frame with the specified dimensions filled with zeros.
/// The frame contains valid BGRA pixel data (4 bytes per pixel).
///
/// # Arguments
///
/// * `width` - Frame width in pixels
/// * `height` - Frame height in pixels
/// * `timestamp` - Timestamp in microseconds (typically QPC ticks)
///
/// # Example
///
/// ```
/// let frame = make_test_frame(1920, 1080, 0);
/// assert_eq!(frame.resolution, (1920, 1080));
/// assert_eq!(frame.bgra.len(), 1920 * 1080 * 4);
/// ```
pub fn make_test_frame(width: u32, height: u32, timestamp: i64) -> CapturedFrame {
    let pixel_count = (width * height * 4) as usize; // BGRA = 4 bytes per pixel
    let bgra = Bytes::from(vec![0u8; pixel_count]);

    CapturedFrame {
        bgra,
        #[cfg(windows)]
        d3d11: None,
        timestamp,
        resolution: (width, height),
    }
}

/// Create a test EncodedPacket with synthetic data.
///
/// Generates an encoded packet with the specified parameters.
/// Useful for testing buffer management and muxing without real encoding.
///
/// # Arguments
///
/// * `pts` - Presentation timestamp in stream timebase units
/// * `is_keyframe` - Whether this packet is a keyframe (I-frame)
/// * `size` - Size of synthetic packet data in bytes
///
/// # Example
///
/// ```
/// let packet = make_test_packet(1000, true, 1024);
/// assert!(packet.is_keyframe);
/// assert_eq!(packet.data.len(), 1024);
/// ```
pub fn make_test_packet(pts: i64, is_keyframe: bool, size: usize) -> EncodedPacket {
    EncodedPacket {
        data: Bytes::from(vec![0u8; size]),
        pts,
        dts: pts,
        is_keyframe,
        stream: StreamType::Video,
        resolution: None,
    }
}

/// Create a test config with sensible defaults for testing.
///
/// Returns a default Config instance suitable for most tests.
/// The config has reasonable values that won't cause excessive
/// memory usage or unrealistic expectations.
pub fn make_default_config() -> liteclip_core::config::Config {
    liteclip_core::config::Config::default()
}

/// Create a config with a specific replay duration.
///
/// # Arguments
///
/// * `duration_secs` - Replay buffer duration in seconds
///
/// # Example
///
/// ```
/// let config = make_config_with_duration(60);
/// assert_eq!(config.general.replay_duration_secs, 60);
/// ```
pub fn make_config_with_duration(duration_secs: u64) -> liteclip_core::config::Config {
    let mut config = make_default_config();
    config.general.replay_duration_secs = duration_secs as u32;
    config
}

/// Create a sequence of test frames simulating video capture.
///
/// Generates `count` frames at the specified framerate with sequential timestamps.
/// Useful for simulating a capture session without actual screen recording.
///
/// # Arguments
///
/// * `count` - Number of frames to generate
/// * `width` - Frame width in pixels
/// * `height` - Frame height in pixels
/// * `framerate` - Framerate in FPS (used to calculate timestamps)
///
/// # Example
///
/// ```
/// let frames = make_frame_sequence(60, 1920, 1080, 30);
/// assert_eq!(frames.len(), 60);
/// // 60 frames at 30fps = 2 seconds worth
/// assert_eq!(frames.last().unwrap().timestamp, 2_000_000); // microseconds
/// ```
pub fn make_frame_sequence(
    count: usize,
    width: u32,
    height: u32,
    framerate: u32,
) -> Vec<CapturedFrame> {
    let interval = 1_000_000 / framerate as i64; // microseconds
    (0..count)
        .map(|i| make_test_frame(width, height, i as i64 * interval))
        .collect()
}

/// Create a sequence of encoded packets with sequential PTS values.
///
/// Generates `count` packets with keyframes at the specified interval.
/// Useful for testing GOP structure and keyframe alignment.
///
/// # Arguments
///
/// * `count` - Number of packets to generate
/// * `pts_interval` - Time between packets in timebase units (microseconds for 1MHz)
/// * `keyframe_interval` - Distance between keyframes (GOP size)
///
/// # Example
///
/// ```
/// // 90 packets at 30fps with keyframe every 30 frames (GOP = 30)
/// let packets = make_packet_sequence(90, 33_333, 30);
/// let keyframes: Vec<_> = packets.iter().enumerate()
///     .filter(|(_, p)| p.is_keyframe)
///     .map(|(i, _)| i)
///     .collect();
/// assert_eq!(keyframes, vec![0, 30, 60]);
/// ```
pub fn make_packet_sequence(
    count: usize,
    pts_interval: i64,
    keyframe_interval: usize,
) -> Vec<EncodedPacket> {
    (0..count)
        .map(|i| make_test_packet(i as i64 * pts_interval, i % keyframe_interval == 0, 1024))
        .collect()
}

/// Minimal valid config TOML string for testing deserialization.
///
/// Contains only the essential fields required for a valid config.
/// Useful for testing TOML parsing and migration scenarios.
pub fn minimal_config_toml() -> String {
    r#"[general]
hotkey_save = "Alt+F1"
hotkey_toggle = "Pause"

[video]
encoder = "auto"
framerate = 60
bitrate = 50000

[advanced]
memory_limit_mb = 512
"#
    .to_string()
}

/// Config TOML with all fields explicitly set for comprehensive testing.
///
/// Contains values for every config field to verify full roundtrip serialization.
pub fn comprehensive_config_toml() -> String {
    r#"[general]
replay_duration_secs = 120
save_directory = "C:\\LiteClip\\Clips"
hotkey_save = "Alt+F10"
hotkey_toggle = "Ctrl+Shift+R"

[video]
encoder = "nvenc"
framerate = 120
bitrate = 80
resolution = "1080p"
quality_preset = "quality"
rate_control = "vbr"

[audio]
enabled = true
input_device = "default"
output_device = "default"
volume_input = 100
volume_output = 100

[hotkeys]
save_clip = "Alt+F10"
toggle_recording = "Ctrl+Shift+R"

[advanced]
memory_limit_mb = 1024
thread_priority = "high"
use_hardware_encoder = true
"#
    .to_string()
}
