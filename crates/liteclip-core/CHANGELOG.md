# Changelog

All notable changes to **liteclip-core** are documented here.

The format is loosely based on [Keep a Changelog](https://keepachangelog.com/).

## Unreleased

### Added

- **`ffmpeg-cli` feature:** mutually exclusive with `ffmpeg`. Records via external `ffmpeg.exe` / `ffprobe` only (no `ffmpeg-next` link), software `libx264` pipe encoder (`encode::cli_pipe`), CLI mux/thumbnail/probe paths.
- **`ffmpeg_backend` module:** `FfmpegBackendKind`, `compiled_backend_kind`, `validate_runtime`, `validate_cli_runtime`, and `FfmpegRuntimeError`.
- **`output::sdk_ffmpeg_output`:** linked-libav remux, probe, thumbnails, and preview (SDK backend; no `ffmpeg.exe`).
- Example `minimal_engine_cli` for the CLI backend.
- `liteclip-replay` re-exports `ffmpeg_backend`; binary calls `validate_runtime` after `init_ffmpeg`.
- `AppDirs` for embedder-specific config and default clip folder layout (`paths` module).
- `Config::{load_with_dirs, save_to_dirs, load_sync_from_dirs, save_sync_to_dirs, default_with_dirs}`; `Config::load` / `save` / `config_path` still use LiteClip Replay defaults.
- `runtime` module: `LITECLIP_CORE_FFMPEG`, `set_ffmpeg_path`, centralized FFmpeg resolution; dev-only `ffmpeg_dev` heuristics gated behind `debug_assertions` or feature `dev-ffmpeg-paths`.
- `CoreHost` trait and optional integration in `AppState` / `ClipManager::save_clip`.
- `ReplayEngine` facade and `prelude` module.
- Examples: `minimal_engine`, `custom_ffmpeg`, `custom_paths`, `engine_host` (`CoreHost` / `set_core_host`).

### Changed

- `ClipManager::save_clip` now takes an optional `Arc<dyn CoreHost>` argument (pass `None` if unused).
- **`ffmpeg` feature** now toggles the optional `ffmpeg-next` dependency. Builds with `default-features = false` omit FFmpeg linking; encoding, native mux (`output::mp4`), and the `encode::ffmpeg` module require `ffmpeg`.
- **SDK vs CLI:** `ffmpeg` and `ffmpeg-cli` are mutually exclusive. Subprocess-only helpers (remux, thumbnails, probe, preview) use linked libav when `ffmpeg` is enabled; they use `ffmpeg`/`ffprobe` when `ffmpeg-cli` is enabled.
- **Removed:** DXGI GPU shader downscaling (HLSL/`fxc`); resize to the configured output resolution is always done in the encoder when it differs from the desktop capture size.
