//! Override FFmpeg resolution with [`liteclip_core::runtime::set_ffmpeg_path`].
//!
//! Run: `cargo run -p liteclip-core --example custom_ffmpeg --features ffmpeg`
//!
//! Note: `set_ffmpeg_path` returns `Err(path)` if the process already set an override (first call wins).
use liteclip_core::output::ffmpeg_executable_path;
use liteclip_core::runtime;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let custom = PathBuf::from(r"C:\path\to\ffmpeg.exe");
    if custom.exists() {
        let _ = runtime::set_ffmpeg_path(custom.clone());
    }

    let resolved = ffmpeg_executable_path();
    println!("Resolved ffmpeg: {}", resolved.display());
    Ok(())
}
