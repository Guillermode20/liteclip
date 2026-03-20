//! Same as `minimal_engine`, but built with **`ffmpeg-cli`** (external `ffmpeg.exe` only).
//!
//! Run: `cargo run -p liteclip-core --example minimal_engine_cli --features ffmpeg-cli --no-default-features`
use liteclip_core::encode;
use liteclip_core::prelude::*;

fn main() -> anyhow::Result<()> {
    encode::init_ffmpeg().map_err(anyhow::Error::from)?;
    liteclip_core::ffmpeg_backend::validate_runtime()?;

    let dirs = AppDirs::from_app_slug("liteclip-embed-example-cli")?;
    let mut engine = ReplayEngine::with_default_config(dirs)?;

    engine.start_recording()?;
    std::thread::sleep(std::time::Duration::from_secs(2));
    engine.stop_recording()?;

    Ok(())
}
