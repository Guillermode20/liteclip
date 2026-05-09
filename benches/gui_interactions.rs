//! Criterion benchmarks for GUI interactions and capture pipeline latency.
//!
//! These benchmarks replace no-op stubs with real benchmarks that exercise
//! LiteClip code paths: config serialization (settings UI), config operations,
//! and a mock capture→encode pipeline round-trip latency.

use bytes::Bytes;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use liteclip::buffer::ring::LockFreeReplayBuffer;
use liteclip::config::Config;
use liteclip::encode::{EncodedPacket, StreamType};

// =============================================================================
// Config / Settings benchmarks
//
// These exercise the config types used by the settings UI in
// `src/gui/settings.rs`.  Serialization / deserialization dominates the
// latency when opening or saving settings.
// =============================================================================

/// Build a realistic config with non-default settings across all sections.
fn make_complex_config() -> Config {
    let mut config = Config::default();
    config.general.replay_duration_secs = 120;
    config.general.save_directory = "C:\\Users\\test\\Videos\\clips".to_string();
    config.general.auto_start_with_windows = true;
    config.video.framerate = 60;
    config.video.bitrate_mbps = 50;
    config.video.encoder = liteclip::config::EncoderType::Software;
    config.video.resolution = liteclip::config::Resolution::P1080;
    config.video.quality_preset = liteclip::config::QualityPreset::Balanced;
    config.audio.system_volume = 192;
    config.audio.mic_volume = 196;
    config.audio.capture_system = true;
    config.audio.capture_mic = true;
    config.advanced.memory_limit_mb = 1024;
    config.hotkeys.save_clip = "Ctrl+Shift+S".to_string();
    config.hotkeys.toggle_recording = "Ctrl+Shift+R".to_string();
    config.hotkeys.open_gallery = "Ctrl+Shift+G".to_string();
    config
}

fn bench_settings_serialize(c: &mut Criterion) {
    c.bench_function("gui/settings_serialization", |b| {
        b.iter(|| {
            let config = make_complex_config();
            black_box(toml::to_string(&config).unwrap())
        })
    });
}

fn bench_settings_deserialize(c: &mut Criterion) {
    let toml_string = toml::to_string(&make_complex_config()).unwrap();

    c.bench_function("gui/settings_deserialization", |b| {
        b.iter(|| black_box(toml::from_str::<Config>(&toml_string).unwrap()))
    });
}

fn bench_settings_roundtrip(c: &mut Criterion) {
    c.bench_function("gui/settings_roundtrip", |b| {
        b.iter(|| {
            let config = make_complex_config();
            let serialized = toml::to_string(&config).unwrap();
            black_box(toml::from_str::<Config>(&serialized).unwrap())
        })
    });
}

fn bench_config_clone(c: &mut Criterion) {
    let config = make_complex_config();

    c.bench_function("gui/config_clone", |b| b.iter(|| black_box(config.clone())));
}

fn bench_config_default(c: &mut Criterion) {
    c.bench_function("gui/config_default", |b| {
        b.iter(|| black_box(Config::default()))
    });
}

// =============================================================================
// Capture pipeline latency benchmark
//
// Simulates the capture→encode→buffer pathway: encoded packets are pushed
// into the ring buffer (as they would be by an encoder thread), then a
// snapshot is taken (as the clip-save or gallery operation would).  Measures
// round-trip latency of the entire path.
// =============================================================================

fn bench_capture_pipeline_latency(c: &mut Criterion) {
    let mut config = Config::default();
    config.general.replay_duration_secs = 30;
    config.advanced.memory_limit_mb = 256;

    let buffer = LockFreeReplayBuffer::new(&config).unwrap();

    // Pre-create packets simulating 10 seconds of captured+encoded video at 30fps
    const FRAME_COUNT: usize = 300;
    const FRAME_INTERVAL: i64 = 33_333_333; // ~30 fps in QPC-based 10 MHz units
    let packets: Vec<EncodedPacket> = (0..FRAME_COUNT)
        .map(|i| EncodedPacket {
            data: Bytes::from(vec![0u8; 4096]), // 4 KB per frame
            pts: i as i64 * FRAME_INTERVAL,
            dts: i as i64 * FRAME_INTERVAL,
            is_keyframe: i % 30 == 0, // GOP = 30
            stream: StreamType::Video,
            resolution: None,
            codec: None,
        })
        .collect();

    c.bench_function("capture/pipeline_latency", |b| {
        b.iter_custom(|iters| {
            let mut total_duration = std::time::Duration::new(0, 0);

            for _ in 0..iters {
                let start = std::time::Instant::now();

                // Phase 1: "Capture" — push all frames into the buffer
                for packet in &packets {
                    buffer.push(packet.clone());
                }

                // Phase 2: "Encode pipeline" — take a snapshot from the start
                let snapshot = buffer.snapshot_from(0);

                let elapsed = start.elapsed();
                total_duration += elapsed;

                // Prevent the optimizer from removing the snapshot
                let _ = black_box(snapshot);
            }

            total_duration
        })
    });
}

criterion_group!(
    gui_benches,
    bench_settings_serialize,
    bench_settings_deserialize,
    bench_settings_roundtrip,
    bench_config_clone,
    bench_config_default,
    bench_capture_pipeline_latency,
);

criterion_main!(gui_benches);
