//! Load/save config using an explicit TOML path via [`liteclip_core::paths::AppDirs::with_config_file`].
//!
//! Run: `cargo run -p liteclip-core --example custom_paths --features ffmpeg`
use liteclip_core::config::Config;
use liteclip_core::paths::AppDirs;

fn main() -> anyhow::Result<()> {
    let base = std::env::temp_dir().join("liteclip-core-custom-paths-example");
    std::fs::create_dir_all(&base)?;
    let config_file = base.join("settings.toml");

    let dirs = AppDirs::with_config_file(config_file.clone(), "demo-clips")?;

    let cfg = Config::load_sync_from_dirs(&dirs)?;
    println!("Loaded config, save_directory = {}", cfg.general.save_directory);

    let mut cfg = cfg;
    cfg.general.replay_duration_secs = cfg.general.replay_duration_secs.max(30);
    cfg.save_sync_to_dirs(&dirs)?;
    println!("Wrote {:?}", dirs.config_file);

    Ok(())
}
