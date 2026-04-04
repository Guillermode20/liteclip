//! Criterion benchmarks for config serialization/deserialization.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use liteclip_core::config::Config;
use tempfile::TempDir;

fn make_complex_config() -> Config {
    let mut config = Config::default();
    config.general.replay_duration_secs = 120;
    config.general.save_directory = "/tmp/clips".to_string();
    config.video.framerate = 60;
    config.video.bitrate_mbps = 50;
    config.advanced.memory_limit_mb = 1024;
    config
}

fn bench_config_serialize(c: &mut Criterion) {
    let config = make_complex_config();

    c.bench_function("config/serialize_to_toml_string", |b| {
        b.iter(|| {
            let config = make_complex_config();
            black_box(toml::to_string(&config).unwrap())
        })
    });
}

fn bench_config_deserialize(c: &mut Criterion) {
    let toml_string = toml::to_string(&make_complex_config()).unwrap();

    c.bench_function("config/deserialize_from_toml_string", |b| {
        b.iter(|| black_box(toml::from_str::<Config>(&toml_string).unwrap()))
    });
}

fn bench_config_roundtrip(c: &mut Criterion) {
    let config = make_complex_config();

    c.bench_function("config/roundtrip_serialize_deserialize", |b| {
        b.iter(|| {
            let config = make_complex_config();
            let serialized = toml::to_string(&config).unwrap();
            black_box(toml::from_str::<Config>(&serialized).unwrap())
        })
    });
}

fn bench_config_file_write(c: &mut Criterion) {
    let config = make_complex_config();

    c.bench_function("config/write_to_file", |b| {
        b.iter(|| {
            let temp_dir = TempDir::new().unwrap();
            let config_file = temp_dir.path().join("settings.toml");
            let content = toml::to_string(&config).unwrap();
            black_box(std::fs::write(&config_file, content).unwrap())
        })
    });
}

fn bench_config_file_read(c: &mut Criterion) {
    let config = make_complex_config();
    let temp_dir = TempDir::new().unwrap();
    let config_file = temp_dir.path().join("settings.toml");
    let content = toml::to_string(&config).unwrap();
    std::fs::write(&config_file, &content).unwrap();

    c.bench_function("config/read_from_file", |b| {
        b.iter(|| {
            let content = black_box(std::fs::read_to_string(&config_file).unwrap());
            black_box(toml::from_str::<Config>(&content).unwrap())
        })
    });
}

criterion_group!(
    config_benches,
    bench_config_serialize,
    bench_config_deserialize,
    bench_config_roundtrip,
    bench_config_file_write,
    bench_config_file_read,
);

criterion_main!(config_benches);
