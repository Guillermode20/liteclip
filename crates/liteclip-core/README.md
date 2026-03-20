# liteclip-core

Windows screen capture, encoding, and replay-buffer engine used by **LiteClip Replay**. Embed it in your own app for a background replay buffer and on-demand clip export.

## FFmpeg: pick one backend (mutually exclusive)

`liteclip-core` does **not** bundle FFmpeg. You (or your users) install it separately. At **compile time** you must enable **exactly one** of:

| Cargo feature | What it is | Typical use |
|----------------|------------|-------------|
| **`ffmpeg`** (default) | **SDK / shared DLLs** via `ffmpeg-next` — hardware encoders (NVENC, AMF, QSV), native muxing, thumbnails, and probing **without** needing `ffmpeg.exe` for those paths. | **Recommended** for production embedders. |
| **`ffmpeg-cli`** | **No** `ffmpeg-next` link. Recording uses `ffmpeg.exe` with **software `libx264`** over pipes; **`ffprobe`** for metadata. | Simpler deployment when you only want users to install a standard FFmpeg **binary** package. |

You cannot enable both: the crate fails to compile if `ffmpeg` and `ffmpeg-cli` are set together.

### Recommended: SDK / DLL path (`ffmpeg` feature)

1. **Build** your app against FFmpeg **dev** artifacts that match the `ffmpeg-next` line in `Cargo.toml` (same major libav version as the DLLs you ship). Point your environment / build scripts at the FFmpeg prefix the way `ffmpeg-sys-next` / your toolchain expects (often `FFMPEG_DIR` on Windows).
2. **Runtime:** place the same major-version **shared DLLs** (e.g. `avcodec-*.dll`, `avformat-*.dll`, `avutil-*.dll`, `swscale-*.dll`, `swresample-*.dll`) **next to your executable** or on the DLL search path. The exact soname suffixes must match the build you linked against.
3. At startup: `liteclip_core::encode::init_ffmpeg()` then `liteclip_core::ffmpeg_backend::validate_runtime()` (SDK validation is a no-op if init succeeded; CLI validation checks `ffmpeg` / `ffprobe`).

This avoids shipping `ffmpeg.exe` for core recording, muxing, thumbnails, and file probing.

### Alternative: CLI-only path (`ffmpeg-cli` feature)

1. **Build** with `--no-default-features --features ffmpeg-cli` (and any other features you need).
2. **Runtime:** users install a full FFmpeg build that includes **`ffmpeg`** and **`ffprobe`** on `PATH`, or place both next to your app. Set `LITECLIP_CORE_FFMPEG` to the `ffmpeg.exe` path if needed, or use `set_ffmpeg_path` (see [`runtime`](https://docs.rs/liteclip-core/latest/liteclip_core/runtime/index.html)).
3. Call `encode::init_ffmpeg()` (no-op for libav) and `ffmpeg_backend::validate_runtime()` before starting the pipeline.
4. **CPU readback** must be enabled so frames reach the encoder as BGRA; hardware GPU encoding is **not** used in this backend.

**Limitations (CLI backend):** clip save muxing **without** audio is fully supported; if your config expects **audio in the MP4**, use the SDK backend (`ffmpeg`) or disable audio for now. Targeted **gallery export** (size-targeted two-pass transcode) still invokes `ffmpeg` as a subprocess in **both** backends.

## Supported embedder path

1. Add the crate with **`ffmpeg`** (default) or **`ffmpeg-cli`** as above — not both.
2. Call `liteclip_core::encode::init_ffmpeg()` and `liteclip_core::ffmpeg_backend::validate_runtime()` before starting the pipeline (when a recording backend feature is enabled).
3. Build [`ReplayEngine`](https://docs.rs/liteclip-core/latest/liteclip_core/struct.ReplayEngine.html) with [`AppDirs::from_app_slug`](https://docs.rs/liteclip-core/latest/liteclip_core/paths/struct.AppDirs.html#method.from_app_slug) (or [`AppDirs::with_config_file`](https://docs.rs/liteclip-core/latest/liteclip_core/paths/struct.AppDirs.html#method.with_config_file)).
4. Use [`ReplayEngine::start_recording`](https://docs.rs/liteclip-core/latest/liteclip_core/struct.ReplayEngine.html#method.start_recording) / [`stop_recording`](https://docs.rs/liteclip-core/latest/liteclip_core/struct.ReplayEngine.html#method.stop_recording), poll [`enforce_pipeline_health`](https://docs.rs/liteclip-core/latest/liteclip_core/struct.ReplayEngine.html#method.enforce_pipeline_health) while running, and call async [`save_clip`](https://docs.rs/liteclip-core/latest/liteclip_core/struct.ReplayEngine.html#method.save_clip) from a **Tokio** runtime.
5. Optional UI hooks: [`CoreHost`](https://docs.rs/liteclip-core/latest/liteclip_core/host/trait.CoreHost.html) via [`ReplayEngine::set_core_host`](https://docs.rs/liteclip-core/latest/liteclip_core/struct.ReplayEngine.html#method.set_core_host) and the `host` argument on `save_clip`.

Rustdoc on `liteclip_core` expands on advanced modules vs the stable facade.

## Requirements

- **OS:** Windows 10+ (`x86_64-pc-windows-msvc` is the supported target). Other targets are not supported for runtime use.
- **FFmpeg:** See the table above. For CLI resolution order, see [`runtime`](https://docs.rs/liteclip-core/latest/liteclip_core/runtime/index.html) (`LITECLIP_CORE_FFMPEG`, `set_ffmpeg_path`, sibling `ffmpeg.exe`, `PATH`, and optional `ffmpeg_dev` heuristics when `debug_assertions` or the `dev-ffmpeg-paths` feature is enabled).

## Features and defaults

- **`ffmpeg` (default)** — SDK backend: `ffmpeg-next`, [`encode::ffmpeg`](https://docs.rs/liteclip-core/latest/liteclip_core/encode/ffmpeg/index.html), native MP4 muxing, linked-libav output helpers (`output::sdk_ffmpeg_output`). With `--no-default-features` only, you get a smaller build for **config I/O and helpers**; recording needs a backend feature.
- **`ffmpeg-cli`** — CLI-only backend; mutually exclusive with `ffmpeg`.
- **`dev-ffmpeg-paths`** — Adds repo `ffmpeg_dev\...` **exe** search heuristics in **release** builds (debug builds already search there when `debug_assertions` is on).

## Examples

| Example | Purpose |
|--------|---------|
| `minimal_engine` | Start/stop recording with a custom app slug (SDK) |
| `minimal_engine_cli` | Same with **`ffmpeg-cli`** |
| `engine_host` | Same as `minimal_engine`, plus `CoreHost` / `set_core_host` |
| `custom_ffmpeg` | Override FFmpeg **binary** resolution with `set_ffmpeg_path` |
| `custom_paths` | Load/save `Config` with a custom TOML path (no FFmpeg init) |

```bash
cargo run -p liteclip-core --example minimal_engine --features ffmpeg
cargo run -p liteclip-core --example minimal_engine_cli --no-default-features --features ffmpeg-cli
cargo run -p liteclip-core --example engine_host --features ffmpeg
cargo run -p liteclip-core --example custom_ffmpeg --features ffmpeg
cargo run -p liteclip-core --example custom_paths
```

`custom_paths` only touches config I/O. Run it with `--no-default-features` to omit FFmpeg backends.

## Versioning

`liteclip-core` is versioned independently of the `liteclip-replay` binary crate. See `CHANGELOG.md`.
