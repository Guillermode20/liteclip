//! Integration: config load/save through [`AppDirs`] without FFmpeg or capture.

use liteclip_core::config::Config;
use liteclip_core::paths::AppDirs;

#[test]
fn config_roundtrip_through_custom_dirs() -> anyhow::Result<()> {
    let base =
        std::env::temp_dir().join(format!("liteclip_core_config_test_{}", std::process::id()));
    std::fs::create_dir_all(&base)?;

    let config_file = base.join("settings.toml");
    let dirs = AppDirs::with_config_file(config_file, "test-clips")?;

    let mut cfg = Config::load_sync_from_dirs(&dirs)?;
    let original_duration = cfg.general.replay_duration_secs;
    cfg.general.replay_duration_secs = original_duration.max(45);
    cfg.save_sync_to_dirs(&dirs)?;

    let cfg2 = Config::load_sync_from_dirs(&dirs)?;
    assert_eq!(
        cfg2.general.replay_duration_secs,
        cfg.general.replay_duration_secs
    );

    std::fs::remove_dir_all(&base)?;
    Ok(())
}
