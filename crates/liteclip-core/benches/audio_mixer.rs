//! Criterion benchmarks for audio mixer operations.

use bytes::Bytes;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use liteclip_core::capture::audio::mixer::AudioMixer;
use liteclip_core::config::AudioConfig;
use liteclip_core::encode::{EncodedPacket, StreamType};

fn make_test_audio_config() -> AudioConfig {
    AudioConfig::default()
}

fn make_packet(pts: i64, size: usize) -> EncodedPacket {
    EncodedPacket {
        data: Bytes::from(vec![0u8; size]),
        pts,
        dts: pts,
        is_keyframe: false,
        stream: StreamType::SystemAudio,
        resolution: None,
        codec: None,
    }
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

    // Pre-populate with some packets
    for i in 0..10 {
        mixer.mix_packets(Some(make_packet(i as i64 * 1_000_000, 4096)), None);
        mixer.mix_packets(None, Some(make_packet(i as i64 * 1_000_000, 4096)));
    }

    c.bench_function("audio/mixing_operation_sync", |b| {
        b.iter(|| black_box(mixer.mix_packets(None, None)))
    });
}

fn bench_push_system(c: &mut Criterion) {
    let config = make_test_audio_config();
    let mut mixer = AudioMixer::new(&config);

    c.bench_function("audio/push_system_packet", |b| {
        b.iter(|| black_box(mixer.mix_packets(Some(make_packet(0, 4096)), None)))
    });
}

fn bench_push_mic(c: &mut Criterion) {
    let config = make_test_audio_config();
    let mut mixer = AudioMixer::new(&config);

    c.bench_function("audio/push_mic_packet", |b| {
        b.iter(|| black_box(mixer.mix_packets(None, Some(make_packet(0, 4096)))))
    });
}

fn bench_drain_queues(c: &mut Criterion) {
    let config = make_test_audio_config();

    c.bench_function("audio/batch_push_and_mix", |b| {
        b.iter(|| {
            let mut m = AudioMixer::new(&config);
            for i in 0..32 {
                m.mix_packets(Some(make_packet(i as i64 * 1_000_000, 4096)), None);
                m.mix_packets(None, Some(make_packet(i as i64 * 1_000_000, 4096)));
            }
            black_box(m.mix_packets(None, None))
        })
    });
}

fn bench_mix_with_skew(c: &mut Criterion) {
    let config = make_test_audio_config();
    let mut mixer = AudioMixer::new(&config);

    // Packets with slight timestamp skew to stress sync logic
    for i in 0..10 {
        mixer.mix_packets(Some(make_packet(i as i64 * 1_000_000 + 5000, 4096)), None);
        mixer.mix_packets(None, Some(make_packet(i as i64 * 1_000_000, 4096)));
    }

    c.bench_function("audio/mixing_with_skew", |b| {
        b.iter(|| black_box(mixer.mix_packets(None, None)))
    });
}

criterion_group!(
    audio_mixer_benches,
    bench_mixer_creation,
    bench_mixer_config_update,
    bench_mixer_status_check,
    bench_mixing_operation,
    bench_push_system,
    bench_push_mic,
    bench_drain_queues,
    bench_mix_with_skew,
);

criterion_main!(audio_mixer_benches);
