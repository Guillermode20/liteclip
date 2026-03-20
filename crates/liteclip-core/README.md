# liteclip-core

Windows screen capture, encoding, and replay-buffer engine used by **LiteClip Replay**.

## Requirements

- Windows 10+
- FFmpeg: encoding/muxing via `ffmpeg-next`; external `ffmpeg.exe` for some mux/clip steps. See rustdoc on `liteclip_core::runtime` for resolution order (`LITECLIP_CORE_FFMPEG`, `set_ffmpeg_path`, bundled `ffmpeg.exe`, etc.).

## Quick use

Add `liteclip-core` to your crate, call `encode::init_ffmpeg()`, then build a `ReplayEngine` with `AppDirs::from_app_slug("my-app")`. See `examples/minimal_engine.rs` in this crate.

## Versioning

`liteclip-core` is versioned independently of the `liteclip-replay` binary crate. See `CHANGELOG.md`.
