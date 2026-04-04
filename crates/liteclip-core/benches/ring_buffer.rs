//! Criterion benchmarks for ring buffer operations.

use bytes::Bytes;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use liteclip_core::buffer::ring::LockFreeReplayBuffer;
use liteclip_core::config::Config;
use liteclip_core::encode::{EncodedPacket, StreamType};

fn make_test_config() -> Config {
    let mut config = Config::default();
    config.general.replay_duration_secs = 60;
    config.advanced.memory_limit_mb = 512;
    config
}

fn make_packet(pts: i64, size: usize) -> EncodedPacket {
    EncodedPacket {
        data: Bytes::from(vec![0u8; size]),
        pts,
        dts: pts,
        is_keyframe: pts % 30_000_000 == 0,
        stream: StreamType::Video,
        resolution: None,
    }
}

fn bench_push_single(c: &mut Criterion) {
    let config = make_test_config();
    let buffer = LockFreeReplayBuffer::new(&config).unwrap();

    c.bench_function("ring_buffer/push_single_packet", |b| {
        b.iter(|| {
            let packet = make_packet(0, 1024);
            black_box(buffer.push(packet));
        })
    });
}

fn bench_push_batch(c: &mut Criterion) {
    let config = make_test_config();
    let buffer = LockFreeReplayBuffer::new(&config).unwrap();

    c.bench_function("ring_buffer/push_100_packets", |b| {
        b.iter(|| {
            for i in 0..100 {
                let packet = make_packet(i as i64 * 1_000_000, 1024);
                black_box(buffer.push(packet));
            }
        })
    });
}

fn bench_snapshot_small(c: &mut Criterion) {
    let config = make_test_config();
    let buffer = LockFreeReplayBuffer::new(&config).unwrap();

    // Push 10 packets
    for i in 0..10 {
        buffer.push(make_packet(i as i64 * 1_000_000, 1024));
    }

    c.bench_function("ring_buffer/snapshot_10_packets", |b| {
        b.iter(|| black_box(buffer.snapshot().unwrap()))
    });
}

fn bench_snapshot_large(c: &mut Criterion) {
    let config = make_test_config();
    let buffer = LockFreeReplayBuffer::new(&config).unwrap();

    // Push 1000 packets
    for i in 0..1000 {
        buffer.push(make_packet(i as i64 * 1_000_000, 2048));
    }

    c.bench_function("ring_buffer/snapshot_1000_packets", |b| {
        b.iter(|| black_box(buffer.snapshot().unwrap()))
    });
}

fn bench_concurrent_push_snapshot(c: &mut Criterion) {
    use std::sync::Arc;
    use std::thread;

    let config = make_test_config();
    let buffer = Arc::new(LockFreeReplayBuffer::new(&config).unwrap());

    c.bench_function("ring_buffer/concurrent_push_and_snapshot", |b| {
        b.iter(|| {
            let buffer_clone = buffer.clone();
            let writer = thread::spawn(move || {
                for i in 0..100 {
                    buffer_clone.push(make_packet(i as i64 * 1_000_000, 1024));
                }
            });

            let buffer_clone = buffer.clone();
            let reader = thread::spawn(move || {
                let mut count = 0;
                for _ in 0..10 {
                    if buffer_clone.snapshot().is_ok() {
                        count += 1;
                    }
                }
                count
            });

            writer.join().unwrap();
            black_box(reader.join().unwrap())
        })
    });
}

fn bench_memory_pressure(c: &mut Criterion) {
    // Small memory limit to force eviction
    let mut config = Config::default();
    config.general.replay_duration_secs = 60;
    config.advanced.memory_limit_mb = 1; // 1 MB limit

    let buffer = LockFreeReplayBuffer::new(&config).unwrap();

    c.bench_function("ring_buffer/push_with_eviction", |b| {
        b.iter(|| {
            for i in 0..200 {
                let packet = make_packet(i as i64 * 1_000_000, 10_000);
                black_box(buffer.push(packet));
            }
        })
    });
}

criterion_group!(
    ring_buffer_benches,
    bench_push_single,
    bench_push_batch,
    bench_snapshot_small,
    bench_snapshot_large,
    bench_concurrent_push_snapshot,
    bench_memory_pressure,
);

criterion_main!(ring_buffer_benches);
