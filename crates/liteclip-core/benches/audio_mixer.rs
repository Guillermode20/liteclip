//! Criterion benchmarks for audio mixer operations.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use liteclip_core::capture::audio::mixer::AudioMixer;
use liteclip_core::config::AudioConfig;

fn make_test_audio_config() -> AudioConfig {
    AudioConfig::default()
}

fn bench_mixer_creation(c: &mut Criterion) {
    let config = make_test_audio_config();

    c.bench_function("audio/mixer_creation", |b| {
        b.iter(|| black_box(AudioMixer::new(&config)))
    });
}

fn bench_mixer_config_update(c: &mut Criterion) {
    let config = make_test_audio_config();
    let mut mixer = AudioMixer::new(&config);

    c.bench_function("audio/mixer_config_update", |b| {
        b.iter(|| {
            let mut new_config = AudioConfig::default();
            new_config.system_volume = 191;
            new_config.mic_volume = 217;
            black_box(mixer.update_config(&new_config))
        })
    });
}

fn bench_mixer_status_check(c: &mut Criterion) {
    let config = make_test_audio_config();
    let mixer = AudioMixer::new(&config);

    c.bench_function("audio/mixer_pending_packet_counts", |b| {
        b.iter(|| black_box(mixer.pending_packet_counts()))
    });
}

fn bench_mixing_operation(c: &mut Criterion) {
    let config = make_test_audio_config();
    let mut mixer = AudioMixer::new(&config);

    c.bench_function("audio/mixing_operation_empty", |b| {
        b.iter(|| {
            // Mix with no pending packets (baseline cost)
            black_box(mixer.mix_packets(0, &mut Vec::new()))
        })
    });
}

criterion_group!(
    audio_benches,
    bench_mixer_creation,
    bench_mixer_config_update,
    bench_mixer_status_check,
    bench_mixing_operation,
);

criterion_main!(audio_benches);
