//! Criterion benchmarks for encoder hot paths.
//!
//! Benchmarks the config resolution, codec name mapping, and keyframe
//! interval calculation — operations that run on every encoder startup
//! and every config change.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use liteclip_core::config::{EncoderType, QualityPreset, RateControl};
use liteclip_core::encode::encoder_mod::{
    EncoderConfig, ResolvedEncoderConfig, ResolvedEncoderType,
};

fn bench_codec_name_resolution(c: &mut Criterion) {
    let variants = [
        ResolvedEncoderType::Nvenc,
        ResolvedEncoderType::Amf,
        ResolvedEncoderType::Qsv,
        ResolvedEncoderType::Software,
    ];

    for variant in &variants {
        let name = format!("encode/codec_name_{:?}", variant);
        c.bench_function(&name, |b| {
            b.iter(|| black_box(variant.ffmpeg_hevc_codec_name()))
        });
    }
}

fn bench_encoder_config_creation(c: &mut Criterion) {
    c.bench_function("encode/config_creation", |b| {
        b.iter(|| {
            black_box(EncoderConfig::new(
                25,
                60,
                (1920, 1080),
                EncoderType::Software,
                2,
            ))
        })
    });
}

fn bench_resolved_config_creation(c: &mut Criterion) {
    c.bench_function("encode/resolved_config_creation", |b| {
        b.iter(|| {
            black_box(ResolvedEncoderConfig {
                bitrate_mbps: 25,
                framerate: 60,
                resolution: (1920, 1080),
                use_native_resolution: false,
                encoder_type: ResolvedEncoderType::Software,
                quality_preset: QualityPreset::Balanced,
                rate_control: RateControl::Vbr,
                quality_value: None,
                keyframe_interval_secs: 2,
                use_cpu_readback: false,
                output_index: 0,
            })
        })
    });
}

fn bench_keyframe_interval_calculation(c: &mut Criterion) {
    let configs = [
        (ResolvedEncoderType::Nvenc, 60, 2),
        (ResolvedEncoderType::Amf, 30, 1),
        (ResolvedEncoderType::Qsv, 120, 5),
        (ResolvedEncoderType::Software, 60, 2),
    ];

    for &(encoder, fps, interval) in &configs {
        let name = format!(
            "encode/keyframe_interval_{:?}_{}fps_{}sec",
            encoder, fps, interval
        );
        let resolved = ResolvedEncoderConfig {
            bitrate_mbps: 25,
            framerate: fps,
            resolution: (1920, 1080),
            use_native_resolution: false,
            encoder_type: encoder,
            quality_preset: QualityPreset::Balanced,
            rate_control: RateControl::Vbr,
            quality_value: None,
            keyframe_interval_secs: interval,
            use_cpu_readback: false,
            output_index: 0,
        };
        c.bench_function(&name, |b| {
            b.iter(|| black_box(resolved.keyframe_interval_frames()))
        });
    }
}

fn bench_gpu_transport_check(c: &mut Criterion) {
    let variants = [
        ResolvedEncoderType::Nvenc,
        ResolvedEncoderType::Amf,
        ResolvedEncoderType::Qsv,
        ResolvedEncoderType::Software,
    ];

    for variant in &variants {
        let name = format!("encode/gpu_transport_check_{:?}", variant);
        let resolved = ResolvedEncoderConfig {
            bitrate_mbps: 25,
            framerate: 60,
            resolution: (1920, 1080),
            use_native_resolution: false,
            encoder_type: *variant,
            quality_preset: QualityPreset::Balanced,
            rate_control: RateControl::Vbr,
            quality_value: None,
            keyframe_interval_secs: 2,
            use_cpu_readback: false,
            output_index: 0,
        };
        c.bench_function(&name, |b| {
            b.iter(|| black_box(resolved.supports_gpu_frame_transport()))
        });
    }
}

fn bench_resolution_encoding(c: &mut Criterion) {
    let resolutions = [
        (1280, 720, "720p"),
        (1920, 1080, "1080p"),
        (2560, 1440, "1440p"),
        (3840, 2160, "4K"),
    ];

    for &(width, height, label) in &resolutions {
        let name = format!("encode/resolution_check_{}", label);
        let config = ResolvedEncoderConfig {
            bitrate_mbps: 25,
            framerate: 60,
            resolution: (width, height),
            use_native_resolution: false,
            encoder_type: ResolvedEncoderType::Software,
            quality_preset: QualityPreset::Balanced,
            rate_control: RateControl::Vbr,
            quality_value: None,
            keyframe_interval_secs: 2,
            use_cpu_readback: false,
            output_index: 0,
        };
        c.bench_function(&name, |b| {
            b.iter(|| {
                let (w, h) = config.resolution;
                black_box((w, h))
            })
        });
    }
}

criterion_group!(
    encoder_benches,
    bench_codec_name_resolution,
    bench_encoder_config_creation,
    bench_resolved_config_creation,
    bench_keyframe_interval_calculation,
    bench_gpu_transport_check,
    bench_resolution_encoding,
);

criterion_main!(encoder_benches);
