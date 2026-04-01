# Contributing to LiteClip Replay

## Quick Start

```bash
git clone https://github.com/Guillermode20/liteclip-recorder.git
cd liteclip-recorder
cargo build --release --features ffmpeg
```

## Prerequisites

- Rust 1.74+ (see `rust-toolchain.toml`)
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

## Questions?

Open an issue for discussion.