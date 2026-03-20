# Changelog

All notable changes to **liteclip-core** are documented here.

The format is loosely based on [Keep a Changelog](https://keepachangelog.com/).

## Unreleased

### Added

- `AppDirs` for embedder-specific config and default clip folder layout (`paths` module).
- `Config::{load_with_dirs, save_to_dirs, load_sync_from_dirs, save_sync_to_dirs, default_with_dirs}`; `Config::load` / `save` / `config_path` still use LiteClip Replay defaults.
- `runtime` module: `LITECLIP_CORE_FFMPEG`, `set_ffmpeg_path`, centralized FFmpeg resolution; dev-only `ffmpeg_dev` heuristics gated behind `debug_assertions` or feature `dev-ffmpeg-paths`.
- `CoreHost` trait and optional integration in `AppState` / `ClipManager::save_clip`.
- `ReplayEngine` facade and `prelude` module.
- Examples: `minimal_engine`, `custom_ffmpeg`, `custom_paths`, `engine_host` (`CoreHost` / `set_core_host`).

### Changed

- `ClipManager::save_clip` now takes an optional `Arc<dyn CoreHost>` argument (pass `None` if unused).
- **`ffmpeg` feature** now toggles the optional `ffmpeg-next` dependency. Builds with `default-features = false` omit FFmpeg linking; encoding, native mux (`output::mp4`), and the `encode::ffmpeg` module require `ffmpeg`.
