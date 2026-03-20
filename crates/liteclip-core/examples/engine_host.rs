//! Same as `minimal_engine`, but wires [`CoreHost`] for pipeline fatals via [`ReplayEngine::set_core_host`].
//!
//! Run: `cargo run -p liteclip-core --example engine_host --features ffmpeg`
use liteclip_core::encode;
use liteclip_core::host::CoreHost;
use liteclip_core::prelude::*;
use std::sync::Arc;

struct NoopHost;

impl CoreHost for NoopHost {}

fn main() -> anyhow::Result<()> {
    encode::init_ffmpeg().map_err(anyhow::Error::from)?;

    let dirs = AppDirs::from_app_slug("liteclip-embed-example")?;
    let mut engine = ReplayEngine::with_default_config(dirs)?;
    engine.set_core_host(Some(Arc::new(NoopHost)));

    engine.start_recording()?;
    std::thread::sleep(std::time::Duration::from_secs(2));
    engine.stop_recording()?;

    Ok(())
}
