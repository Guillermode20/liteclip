//! E2E: Full recording loop — synthetic frames → libx265 encode → buffer → save → verify.
//!
//! Tests the complete pipeline end-to-end without requiring DXGI capture or GPU hardware.
//! Uses the FFmpeg software encoder (libx265) to produce real HEVC packets from synthetic
//! BGRA frames with visual patterns, pushes them through the replay buffer, saves via
//! [`ClipManager`](liteclip_core::app::ClipManager), and verifies the output MP4.
//!
//! # Requirements
//!
//! - `ffmpeg` feature (default on). The linked FFmpeg build must include `libx265`.
//! - FFmpeg shared DLLs next to the test executable (see AGENTS.md).
//! - No real display, GPU, or DXGI needed — runs in headless CI.

#![cfg(feature = "ffmpeg")]

mod common;

use std::path::Path;

use anyhow::{Context, Result};
use bytes::Bytes;
use tempfile::TempDir;

use liteclip_core::app::ClipManager;
use liteclip_core::buffer::ring::{qpc_frequency, SharedReplayBuffer};
use liteclip_core::config::{Config, EncoderType, QualityPreset, RateControl};
use liteclip_core::encode::{
    ffmpeg::FfmpegEncoder, Encoder, ResolvedEncoderConfig, ResolvedEncoderType,
};
use liteclip_core::media::CapturedFrame;
use liteclip_core::output::video_file::probe_video_file;
use liteclip_core::paths::AppDirs;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create a minimal test [`Config`] pointing into a temp directory.
fn test_config(temp: &TempDir) -> Result<Config> {
    let clips_dir = temp.path().join("clips");
    std::fs::create_dir_all(&clips_dir)?;

    let config_file = temp.path().join("config.toml");
    let dirs = AppDirs::with_config_file(config_file, "e2e-test")?;
    let mut config = Config::default_with_dirs(&dirs);
    config.general.replay_duration_secs = 5;
    config.video.framerate = 30;
    config.video.bitrate_mbps = 10;
    config.video.encoder = EncoderType::Software;
    config.advanced.memory_limit_mb = 512;
    config.general.save_directory = clips_dir.to_string_lossy().to_string();
    config.general.generate_clip_thumbnail = false;
    config.audio.capture_system = false;
    config.audio.capture_mic = false;
    // Audio disabled explicitly by setting both captures to false
    // (already done above — keep as documentation)
    debug_assert!(
        !config.audio.capture_system && !config.audio.capture_mic,
        "audio capture should be disabled for this test"
    );
    Ok(config)
}

/// A [`ResolvedEncoderConfig`] for software (libx265) at 720p30.
fn software_encoder_config() -> ResolvedEncoderConfig {
    ResolvedEncoderConfig {
        bitrate_mbps: 10,
        framerate: 30,
        resolution: (1280, 720),
        use_native_resolution: false,
        encoder_type: ResolvedEncoderType::Software,
        quality_preset: QualityPreset::Balanced,
        rate_control: RateControl::Cbr,
        quality_value: None,
        keyframe_interval_secs: 2,
        use_cpu_readback: true,
        output_index: 0,
    }
}

/// Generate a synthetic BGRA frame with a time-varying colour gradient.
///
/// A horizontal + vertical gradient is modulated by a per-frame phase so
/// consecutive frames differ — this ensures the encoder produces distinct
/// packets and the output file has actual content (not a single solid colour).
fn make_synthetic_frame(
    width: u32,
    height: u32,
    timestamp: i64,
    frame_index: i32,
) -> CapturedFrame {
    let pixel_count = (width * height * 4) as usize;
    let mut bgra = vec![0u8; pixel_count];

    // Time-varying phases that create apparent motion across frames
    let phase_r = (frame_index as f32 * 0.05).sin() * 0.5 + 0.5;
    let phase_g = (frame_index as f32 * 0.03 + 1.2).cos() * 0.5 + 0.5;
    let phase_b = ((frame_index as f32 * 0.04 + 2.4).sin() * 0.5 + 0.5).max(0.3);

    for y in 0..height {
        let y_norm = y as f32 / (height.saturating_sub(1).max(1) as f32);
        for x in 0..width {
            let x_norm = x as f32 / (width.saturating_sub(1).max(1) as f32);
            let idx = ((y * width + x) * 4) as usize;

            // RGB channels vary spatially AND temporally
            let r = ((x_norm * 255.0 * (0.7 + 0.3 * phase_r)) as u8).min(255);
            let g = ((y_norm * 255.0 * (0.7 + 0.3 * phase_g)) as u8).min(255);
            let b = (((1.0 - x_norm) * 200.0 * phase_b) as u8).min(255);

            // BGRA byte order
            bgra[idx] = b;
            bgra[idx + 1] = g;
            bgra[idx + 2] = r;
            bgra[idx + 3] = 255;
        }
    }

    CapturedFrame {
        bgra: Bytes::from(bgra),
        #[cfg(windows)]
        d3d11: None,
        timestamp,
        resolution: (width, height),
    }
}

/// Decode the first video frame and assert it is **not** uniform/black.
///
/// Reads the file with `ffmpeg-next`, sends the first video packet through
/// the decoder, and computes the luma (Y) plane mean and variance.
///
/// # Panics
///
/// If the frame is all-black (mean luma ≤ 10) or uniform (variance ≤ 100).
fn verify_first_frame_not_black(path: &Path) -> Result<()> {
    use ffmpeg::media::Type;
    use ffmpeg_next as ffmpeg;

    let mut ictx = ffmpeg::format::input(path)
        .with_context(|| format!("failed to open {:?} for frame verification", path.display()))?;

    // Resolve the best video stream *index* before entering the packet loop
    // so we don't hold an immutable reference into `ictx` while iterating.
    let video_stream_index = ictx
        .streams()
        .best(Type::Video)
        .context("no video stream in output file")?
        .index();

    // Build the decoder from stream parameters.
    let stream = ictx
        .stream(video_stream_index)
        .context("missing video stream")?;
    let decoder_ctx = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
    let mut decoder = decoder_ctx
        .decoder()
        .video()
        .context("failed to create video decoder")?;

    for (stream, packet) in ictx.packets() {
        if stream.index() != video_stream_index {
            continue;
        }
        decoder.send_packet(&packet)?;
        let mut frame = ffmpeg::frame::Video::empty();
        if decoder.receive_frame(&mut frame).is_ok() {
            let luma = frame.data(0);
            let len = luma.len();
            anyhow::ensure!(len > 0, "decoded frame has no pixel data");

            // Mean and variance of the luma (Y) plane.
            let (sum, sum_sq) = luma.iter().fold((0u64, 0u64), |(s, sq), &p| {
                (s + p as u64, sq + (p as u64).pow(2))
            });
            let n = len as u64;
            let mean = sum as f64 / n as f64;
            let variance = (sum_sq as f64 / n as f64) - (mean * mean);

            assert!(
                variance > 100.0,
                "First frame appears uniform/blank: variance={:.2}, mean_luma={:.2} \
                 (expected variance > 100 for non-uniform content)",
                variance,
                mean
            );
            assert!(
                mean > 10.0,
                "First frame luma is too dark: mean_luma={:.2} (expected > 10)",
                mean
            );
            return Ok(());
        }
    }

    anyhow::bail!("could not decode any video frame from {:?}", path)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// **Full E2E:** Encode 5 seconds of synthetic video, save, verify.
///
/// Covers:
/// 1. `ffmpeg-next` initialisation
/// 2. `FfmpegEncoder` with `libx265` (software HEVC)
/// 3. `SharedReplayBuffer` ingestion and statistics
/// 4. `ClipManager::save_clip` (async, full mux via `FfmpegMuxer`)
/// 5. `probe_video_file` metadata validation
/// 6. Decode first frame → confirm non-black / non-uniform content
#[test]
fn e2e_full_recording_loop() -> Result<()> {
    // ── Initialise ────────────────────────────────────────────────────
    ffmpeg_next::init().ok(); // Safe to call multiple times (uses Once internally).

    let temp = TempDir::new()?;
    let config = test_config(&temp)?;
    let buffer = SharedReplayBuffer::new(&config)?;
    let enc_cfg = software_encoder_config();

    let mut encoder = FfmpegEncoder::new(&enc_cfg)?;
    encoder.init(&enc_cfg)?;

    // ── Encode 150 frames (~5 s at 30 fps) ────────────────────────────
    let fps = 30;
    let total_frames = fps * config.general.replay_duration_secs as i32; // 150
    let freq = qpc_frequency().max(1);
    let frame_interval = freq / fps as i64;

    for i in 0..total_frames {
        let frame = make_synthetic_frame(1280, 720, i as i64 * frame_interval, i);
        encoder.encode_frame(&frame)?;

        // Drain every encoded packet and feed the buffer.
        while let Ok(pkt) = encoder.packet_rx().try_recv() {
            buffer.push(pkt);
        }
    }

    // Flush encoder and drain remaining packets.
    let remaining = encoder.flush()?;
    for pkt in remaining {
        buffer.push(pkt);
    }
    while let Ok(pkt) = encoder.packet_rx().try_recv() {
        buffer.push(pkt);
    }

    drop(encoder); // Explicit drop — encoder thread / resources released.

    // ── Verify buffer has content ──────────────────────────────────────
    let stats = buffer.stats();
    assert!(
        stats.packet_count > 0,
        "Buffer should contain packets after encoding (got {})",
        stats.packet_count
    );
    assert!(
        stats.keyframe_count > 0,
        "Buffer should contain keyframes (got {})",
        stats.keyframe_count
    );
    assert!(
        stats.total_bytes > 10_000,
        "Buffer should contain more than 10 KB of data (got {} bytes)",
        stats.total_bytes
    );

    // ── Save clip through the full ClipManager pipeline ────────────────
    let rt = tokio::runtime::Runtime::new()?;
    let clip_path =
        rt.block_on(async { ClipManager::save_clip(&config, &buffer, None, None).await })?;

    // Buffer was restarted by save_clip — nothing more to do with it.
    drop(buffer);

    // ── Verification ───────────────────────────────────────────────────

    // 1. File exists with plausible size.
    assert!(
        clip_path.exists(),
        "Clip file should exist at {:?}",
        clip_path
    );
    let file_size = clip_path.metadata()?.len();
    assert!(
        file_size > 50_000,
        "Clip should be larger than 50 KB (got {} bytes)",
        file_size
    );

    // 2. Probe with ffmpeg-next (container + stream metadata).
    let meta = probe_video_file(&clip_path)?;
    assert_eq!(
        meta.width, 1280,
        "Expected 1280 px width, got {}",
        meta.width
    );
    assert_eq!(
        meta.height, 720,
        "Expected 720 px height, got {}",
        meta.height
    );
    assert!(
        meta.duration_secs >= 4.0,
        "Duration should be at least ~5 s (got {:.2} s)",
        meta.duration_secs
    );
    assert!(
        meta.fps > 0.0 && meta.fps <= 60.0,
        "FPS should be in (0,60], got {:.1}",
        meta.fps
    );
    assert!(!meta.has_audio, "Should have no audio streams");

    // 3. Decode first frame → confirm non-black / non-uniform.
    verify_first_frame_not_black(&clip_path)?;

    Ok(())
}

/// **Regression guard:** Short 2-second clip to catch buffer / mux issues at
/// boundary conditions (minimum GOP alignment, single keyframe, etc.).
#[test]
fn e2e_short_clip_boundary() -> Result<()> {
    ffmpeg_next::init().ok();

    let temp = TempDir::new()?;
    let mut config = test_config(&temp)?;
    config.general.replay_duration_secs = 2; // shorter
    config.video.framerate = 30;
    let buffer = SharedReplayBuffer::new(&config)?;
    let enc_cfg = software_encoder_config();

    let mut encoder = FfmpegEncoder::new(&enc_cfg)?;
    encoder.init(&enc_cfg)?;

    let total_frames = 30 * 2; // 60 frames = 2 seconds
    let freq = qpc_frequency().max(1);
    let frame_interval = freq / 30;

    for i in 0..total_frames {
        let frame = make_synthetic_frame(1280, 720, i as i64 * frame_interval, i);
        encoder.encode_frame(&frame)?;
        while let Ok(pkt) = encoder.packet_rx().try_recv() {
            buffer.push(pkt);
        }
    }

    let remaining = encoder.flush()?;
    for pkt in remaining {
        buffer.push(pkt);
    }
    while let Ok(pkt) = encoder.packet_rx().try_recv() {
        buffer.push(pkt);
    }

    let stats = buffer.stats();
    assert!(
        stats.keyframe_count > 0,
        "Buffer must have at least one keyframe"
    );

    let rt = tokio::runtime::Runtime::new()?;
    let clip_path =
        rt.block_on(async { ClipManager::save_clip(&config, &buffer, None, None).await })?;

    assert!(clip_path.exists());
    let file_size = clip_path.metadata()?.len();
    assert!(
        file_size > 20_000,
        "Short clip should be > 20 KB (got {} B)",
        file_size
    );

    let meta = probe_video_file(&clip_path)?;
    assert_eq!(meta.width, 1280, "Width mismatch for short clip");
    assert_eq!(meta.height, 720, "Height mismatch for short clip");

    verify_first_frame_not_black(&clip_path)?;

    Ok(())
}

/// **Regression guard:** Validate that a clip with custom resolution
/// (1920×1080) also produces correct output.
#[test]
fn e2e_higher_resolution() -> Result<()> {
    ffmpeg_next::init().ok();

    let temp = TempDir::new()?;
    let config = test_config(&temp)?;
    let buffer = SharedReplayBuffer::new(&config)?;

    let enc_cfg = ResolvedEncoderConfig {
        resolution: (1920, 1080),
        ..software_encoder_config()
    };

    let mut encoder = FfmpegEncoder::new(&enc_cfg)?;
    encoder.init(&enc_cfg)?;

    let total_frames = 60; // 2 seconds
    let freq = qpc_frequency().max(1);
    let frame_interval = freq / 30;

    for i in 0..total_frames {
        let frame = make_synthetic_frame(1920, 1080, i as i64 * frame_interval, i);
        encoder.encode_frame(&frame)?;
        while let Ok(pkt) = encoder.packet_rx().try_recv() {
            buffer.push(pkt);
        }
    }

    let remaining = encoder.flush()?;
    for pkt in remaining {
        buffer.push(pkt);
    }
    while let Ok(pkt) = encoder.packet_rx().try_recv() {
        buffer.push(pkt);
    }

    let stats = buffer.stats();
    assert!(stats.keyframe_count > 0, "No keyframes in 1080p clip");

    let rt = tokio::runtime::Runtime::new()?;
    let clip_path =
        rt.block_on(async { ClipManager::save_clip(&config, &buffer, None, None).await })?;

    let meta = probe_video_file(&clip_path)?;
    assert_eq!(meta.width, 1920, "1080p width mismatch");
    assert_eq!(meta.height, 1080, "1080p height mismatch");

    verify_first_frame_not_black(&clip_path)?;

    Ok(())
}

/// **Sanity:** Pushing zero-data into the buffer should produce a sensible
/// error (not panic) when trying to save.
#[test]
fn e2e_empty_buffer_rejected() {
    ffmpeg_next::init().ok();

    let temp = TempDir::new().unwrap();
    let config = test_config(&temp).unwrap();
    let buffer = SharedReplayBuffer::new(&config).unwrap();

    // No packets pushed at all.
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(async { ClipManager::save_clip(&config, &buffer, None, None).await });

    assert!(
        result.is_err(),
        "Saving an empty buffer should fail, got Ok"
    );
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("empty") || err.contains("no frames") || err.contains("keyframe"),
        "Error message should describe why save failed: got '{}'",
        err
    );
}
