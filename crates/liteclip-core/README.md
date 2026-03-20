# liteclip-core

Windows screen capture, encoding, and replay-buffer engine used by **LiteClip Replay**. Embed it in your own app for a background replay buffer and on-demand clip export.

## Supported embedder path

1. Add the crate (usually with default features so `ffmpeg` is enabled).
2. Call `liteclip_core::encode::init_ffmpeg()` before starting the pipeline when the `ffmpeg` feature is on.
3. Build [`ReplayEngine`](https://docs.rs/liteclip-core/latest/liteclip_core/struct.ReplayEngine.html) with [`AppDirs::from_app_slug`](https://docs.rs/liteclip-core/latest/liteclip_core/paths/struct.AppDirs.html#method.from_app_slug) (or [`AppDirs::with_config_file`](https://docs.rs/liteclip-core/latest/liteclip_core/paths/struct.AppDirs.html#method.with_config_file)).
4. Use [`ReplayEngine::start_recording`](https://docs.rs/liteclip-core/latest/liteclip_core/struct.ReplayEngine.html#method.start_recording) / [`stop_recording`](https://docs.rs/liteclip-core/latest/liteclip_core/struct.ReplayEngine.html#method.stop_recording), poll [`enforce_pipeline_health`](https://docs.rs/liteclip-core/latest/liteclip_core/struct.ReplayEngine.html#method.enforce_pipeline_health) while running, and call async [`save_clip`](https://docs.rs/liteclip-core/latest/liteclip_core/struct.ReplayEngine.html#method.save_clip) from a **Tokio** runtime.
5. Optional UI hooks: [`CoreHost`](https://docs.rs/liteclip-core/latest/liteclip_core/host/trait.CoreHost.html) via [`ReplayEngine::set_core_host`](https://docs.rs/liteclip-core/latest/liteclip_core/struct.ReplayEngine.html#method.set_core_host) and the `host` argument on `save_clip`.

Rustdoc on `liteclip_core` expands on advanced modules vs the stable facade.

## Requirements

- **OS:** Windows 10+ (`x86_64-pc-windows-msvc` is the supported target). Other targets are not supported for runtime use.
- **FFmpeg:** `ffmpeg-next` links FFmpeg libraries at build time; some flows also invoke an `ffmpeg.exe` process. See [`liteclip_core::runtime`](https://docs.rs/liteclip-core/latest/liteclip_core/runtime/index.html) for resolution order (`LITECLIP_CORE_FFMPEG`, `set_ffmpeg_path`, bundled `ffmpeg.exe`, `PATH`, and optional `ffmpeg_dev` heuristics when `debug_assertions` or the `dev-ffmpeg-paths` feature is enabled).
- **Build:** Optional HLSL compilation uses the Windows SDK `fxc.exe` when present. If `fxc` is missing, GPU shader scaling is disabled (see `build.rs` warnings).

## Features and defaults

- **`ffmpeg` (default)** — Enables the optional `ffmpeg-next` dependency, the [`encode::ffmpeg`](https://docs.rs/liteclip-core/latest/liteclip_core/encode/ffmpeg/index.html) module, and native MP4 muxing. Without it (`--no-default-features`), you get a smaller build suitable for **config I/O and helpers only**; recording and saving clips need `ffmpeg`.
- The **LiteClip Replay** workspace crate disables default features on `liteclip-core` and enables `ffmpeg` explicitly; standalone embedders often use `default-features = true` or `features = ["ffmpeg"]`.
- **`dev-ffmpeg-paths`** — Adds repo `ffmpeg_dev\...` search heuristics in **release** builds (debug builds already search there when `debug_assertions` is on).

## Examples

| Example | Purpose |
|--------|---------|
| `minimal_engine` | Start/stop recording with a custom app slug |
| `engine_host` | Same, plus `CoreHost` / `set_core_host` |
| `custom_ffmpeg` | Override FFmpeg resolution with `set_ffmpeg_path` |
| `custom_paths` | Load/save `Config` with a custom TOML path (no FFmpeg init) |

```bash
cargo run -p liteclip-core --example minimal_engine --features ffmpeg
cargo run -p liteclip-core --example engine_host --features ffmpeg
cargo run -p liteclip-core --example custom_ffmpeg --features ffmpeg
cargo run -p liteclip-core --example custom_paths
```

`custom_paths` only touches config I/O. You can run it with `--no-default-features` to avoid linking `ffmpeg-next` (see **Features and defaults** above).

## Versioning

`liteclip-core` is versioned independently of the `liteclip-replay` binary crate. See `CHANGELOG.md`.
