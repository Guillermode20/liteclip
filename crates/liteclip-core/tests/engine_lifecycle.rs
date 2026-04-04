//! Integration: End-to-end ReplayEngine lifecycle tests.
//!
//! Tests the full engine lifecycle from creation through recording to clip saving,
//! verifying that all components work together correctly.

mod common;

use common::fixtures::make_packet_sequence;
use liteclip_core::config::Config;
use liteclip_core::host::CoreHost;
use liteclip_core::paths::AppDirs;
use liteclip_core::ReplayEngine;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tempfile::TempDir;

#[test]
fn engine_builder_creates_valid_engine() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let config_file = temp.path().join("config.toml");
    let dirs = AppDirs::with_config_file(config_file, "test-e2e")?;
    let engine = ReplayEngine::builder(dirs).build()?;

    let stats = engine.state().replay_buffer_stats();
    assert_eq!(stats.packet_count, 0);
    Ok(())
}

#[test]
fn engine_start_stop_lifecycle() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let config_file = temp.path().join("config.toml");
    let dirs = AppDirs::with_config_file(config_file, "test-e2e")?;
    let mut engine = ReplayEngine::builder(dirs).build()?;

    assert!(!engine.state().is_recording());
    engine.start_recording()?;
    assert!(engine.state().is_recording());

    let packets = make_packet_sequence(10, 1_000_000 / 30, 10);
    let (_config, buffer) = engine.state().save_context();
    for packet in &packets {
        buffer.push(packet.clone());
    }

    engine.stop_recording()?;
    assert!(!engine.state().is_recording());

    let snapshot = buffer.snapshot()?;
    assert!(
        !snapshot.as_slice().is_empty(),
        "Buffer should contain packets"
    );

    Ok(())
}

#[test]
fn engine_save_context_returns_buffer_and_config() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let config_file = temp.path().join("config.toml");
    let dirs = AppDirs::with_config_file(config_file, "test-e2e")?;
    let mut engine = ReplayEngine::builder(dirs).build()?;

    engine.start_recording()?;

    let packets = make_packet_sequence(5, 1_000_000 / 30, 5);
    let (config, buffer) = engine.state().save_context();
    for packet in &packets {
        buffer.push(packet.clone());
    }

    assert_eq!(config.general.replay_duration_secs, 30);
    let snap = buffer.snapshot()?;
    assert!(!snap.as_slice().is_empty());

    Ok(())
}

#[test]
fn core_host_wiring() -> anyhow::Result<()> {
    struct RecordingHost {
        saved_count: AtomicUsize,
    }

    impl CoreHost for RecordingHost {
        fn on_clip_saved(&self, _path: &Path) {
            self.saved_count.fetch_add(1, Ordering::SeqCst);
        }
    }

    let temp = TempDir::new()?;
    let config_file = temp.path().join("config.toml");
    let dirs = AppDirs::with_config_file(config_file, "test-e2e")?;
    let host = Arc::new(RecordingHost {
        saved_count: AtomicUsize::new(0),
    });

    let mut engine = ReplayEngine::builder(dirs)
        .with_host(host.clone())
        .build()?;

    assert!(engine.core_host().is_some());

    engine.start_recording()?;
    engine.stop_recording()?;

    Ok(())
}

#[test]
fn engine_enforce_pipeline_health_returns_none_when_idle() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let config_file = temp.path().join("config.toml");
    let dirs = AppDirs::with_config_file(config_file, "test-e2e")?;
    let mut engine = ReplayEngine::builder(dirs).build()?;

    let health = engine.enforce_pipeline_health()?;
    assert!(health.is_none());

    engine.start_recording()?;
    let health = engine.enforce_pipeline_health()?;
    assert!(health.is_none());

    Ok(())
}

#[test]
fn engine_config_validation_applied() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let config_file = temp.path().join("config.toml");
    let dirs = AppDirs::with_config_file(config_file, "test-e2e")?;
    let mut config = Config::default();
    config.general.replay_duration_secs = 60;
    config.video.framerate = 60;

    let engine = ReplayEngine::builder(dirs).with_config(config).build()?;

    let (loaded_config, _) = engine.state().save_context();
    assert_eq!(loaded_config.general.replay_duration_secs, 60);
    assert_eq!(loaded_config.video.framerate, 60);

    Ok(())
}
