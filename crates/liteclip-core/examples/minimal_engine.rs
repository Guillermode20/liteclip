//! Start and stop the recording pipeline with a custom app slug.
//!
//! Run: `cargo run -p liteclip-core --example minimal_engine --features ffmpeg`
use liteclip_core::encode;
use liteclip_core::prelude::*;

fn main() -> anyhow::Result<()> {
    encode::init_ffmpeg().map_err(anyhow::Error::from)?;

    let dirs = AppDirs::from_app_slug("liteclip-embed-example")?;
    let mut engine = ReplayEngine::with_default_config(dirs)?;

    engine.start_recording()?;
    std::thread::sleep(std::time::Duration::from_secs(2));
    engine.stop_recording()?;

    Ok(())
}
