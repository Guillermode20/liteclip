# Contributing to LiteClip Replay

## Quick Start

```bash
git clone https://github.com/Guillermode20/liteclip-recorder.git
cd liteclip-recorder
cargo build --release --features ffmpeg
```

## Prerequisites

- Rust 1.94+ (see `rust-toolchain.toml`)
- FFmpeg 6.0+ shared libraries
- Windows SDK + Visual Studio Build Tools 2022

## Code Style

```bash
cargo fmt --check
cargo clippy -- -D warnings
cargo test
```

- Document all `pub` items
- Use `Result` over `unwrap`
- Add `# Safety` docs for `unsafe` blocks

## PR Process

1. Fork and branch from `master`
2. Add tests for new functionality
3. Run the checks above
4. Open a pull request

Commit format: `type(scope): description` (e.g., `fix(capture): handle DXGI access lost`)

## Debugging

```powershell
$env:RUST_LOG = "debug,liteclip_core=trace,wgpu=warn,naga=warn"
cargo run
```

### Common Issues

- **DXGI_ACCESS_LOST**: Expected on secure desktop (UAC, lock screen). Capture thread handles reacquisition — don't panic.
- **Hardware encoder fallback**: Check logs for unexpected CPU fallback. NVENC/AMF/QSV fall back to software when unavailable.
- **FFmpeg DLL not found**: Place `avcodec-*.dll`, `avformat-*.dll`, `avutil-*.dll`, `swscale-*.dll`, `swresample-*.dll`, `avfilter-*.dll` next to the executable.

## Hardware Encoders (NVENC / QSV / AMF)

PRs that change encoder behavior should include GPU model, driver notes, and relevant `tracing` output. Key source locations:

- Hub: `crates/liteclip-core/src/encode/ffmpeg/mod.rs`
- NVENC: `crates/liteclip-core/src/encode/ffmpeg/nvenc.rs`
- QSV: `crates/liteclip-core/src/encode/ffmpeg/qsv.rs`
- AMF: `crates/liteclip-core/src/encode/ffmpeg/amf.rs`
- Auto-detect: `crates/liteclip-core/src/encode/encoder_mod/functions.rs`

See [AGENTS.md](AGENTS.md) for full architecture details, threading model, and memory management patterns.

## Questions?

Open an issue for discussion.