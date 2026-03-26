# Contributing to LiteClip Replay

Thank you for your interest in contributing! This document provides guidelines and instructions for contributing to LiteClip Replay.

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Development Setup](#development-setup)
- [Building](#building)
- [Code Style](#code-style)
- [Documentation](#documentation)
- [Testing](#testing)
- [Pull Request Process](#pull-request-process)
- [Commit Messages](#commit-messages)
- [Architecture Overview](#architecture-overview)
  - [Hardware encoders (NVENC / Intel QSV)](#hardware-encoders-nvenc--intel-qsv)

## Code of Conduct

Be respectful and constructive. We welcome contributions from everyone.

## Development Setup

### Prerequisites

| Requirement | Version | Notes |
|-------------|---------|-------|
| Rust | 1.70+ | Use `rustup` for installation |
| FFmpeg | 6.0+ | Shared libraries required |
| Windows SDK | 10+ | For Windows API bindings |
| Visual Studio Build Tools | 2022 | C++ workload required |

### Installing Rust

```powershell
# Install rustup
winget install Rustlang.Rustup

# Or via the installer from https://rustup.rs
```

### FFmpeg Setup

1. Download FFmpeg shared builds from [gyan.dev](https://www.gyan.dev/ffmpeg/builds/) or [BtbN](https://github.com/BtbN/FFmpeg-Builds/releases)
2. Extract to a directory (e.g., `C:\ffmpeg`)
3. Add the `bin` directory to your `PATH`:
   ```powershell
   $env:PATH += ";C:\ffmpeg\bin"
   ```
4. Verify:
   ```powershell
   ffmpeg -version
   ```

### Clone and Setup

```bash
git clone https://github.com/your-repo/liteclip-recorder.git
cd liteclip-recorder
cargo fetch
```

## Building

### Debug Build

```bash
cargo build
```

### Release Build

```bash
cargo build --release --features ffmpeg
```

### Run

```bash
# Debug (with console output)
cargo run

# Release
cargo run --release --features ffmpeg
```

## Code Style

### Formatting

We use `rustfmt` for consistent formatting:

```bash
# Check formatting
cargo fmt --check

# Apply formatting
cargo fmt
```

### Linting

We use `clippy` for linting:

```bash
# Run clippy
cargo clippy -- -D warnings
```

All code must pass `clippy` with no warnings.

### Guidelines

1. **Public items must be documented**: All `pub` items require documentation comments (`///`)
2. **Prefer `Result` over `unwrap`**: Handle errors explicitly
3. **Use `tracing` for logging**: Use `info!`, `warn!`, `error!`, `debug!`, `trace!`
4. **Follow Rust naming conventions**: `snake_case` for functions/variables, `PascalCase` for types
5. **Keep functions focused**: Each function should do one thing well
6. **Avoid deep nesting**: Use early returns and guard clauses

## Documentation

### Module Documentation

Every module (`mod.rs`) must have a module-level doc comment:

```rust
//! Module Name - Brief Description
//!
//! Longer description of what this module does and its role in the
//! application architecture.
//!
//! # Key Types
//!
//! - [`TypeName`] - Description of what this type does
//!
//! # Example
//!
//! ```
//! use crate::module::TypeName;
//! let instance = TypeName::new();
//! ```
```

### Item Documentation

All public items require documentation:

```rust
/// Brief description of the function.
///
/// # Arguments
///
/// * `param1` - Description of first parameter
/// * `param2` - Description of second parameter
///
/// # Returns
///
/// Description of what is returned.
///
/// # Errors
///
/// Description of when this function can fail.
///
/// # Example
///
/// ```
/// use crate::module::function_name;
/// let result = function_name("arg1", "arg2");
/// assert!(result.is_ok());
/// ```
pub fn function_name(param1: &str, param2: &str) -> Result<()> {
    // ...
}
```

### Safety Documentation

Any `unsafe` code must include a `# Safety` section:

```rust
/// Performs an unsafe operation.
///
/// # Safety
///
/// - Caller must ensure pointer is valid
/// - Caller must ensure proper alignment
pub unsafe fn unsafe_function(ptr: *const u8) {
    // ...
}
```

## Testing

### Running Tests

```bash
# Run all tests
cargo test

# Run specific test
cargo test test_name

# Run tests with output
cargo test -- --nocapture
```

### Writing Tests

- Place unit tests in the same file with `#[cfg(test)]` module
- Place integration tests in `tests/` directory
- Use descriptive test names: `test_<function>_<scenario>_<expected_result>`

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frame_duration_60fps_returns_correct_value() {
        let duration = frame_duration(60);
        assert_eq!(duration.as_nanos(), 16_666_666);
    }

    #[test]
    fn test_frame_duration_zero_fps_clamps_to_minimum() {
        let duration = frame_duration(0);
        assert_eq!(duration.as_nanos(), 1_000_000_000);
    }
}
```

## Pull Request Process

1. **Fork the repository** and create your branch from `master`
2. **Make your changes** following the code style guidelines
3. **Add tests** for new functionality
4. **Update documentation** for changed public APIs
5. **Run checks locally**:
   ```bash
   cargo fmt --check
   cargo clippy -- -D warnings
   cargo test
   cargo build --release --features ffmpeg
   ```
6. **Commit your changes** (see [Commit Messages](#commit-messages))
7. **Push to your fork** and open a pull request
8. **Address review feedback** promptly

### Pre-commit Hooks

The repository includes a pre-commit hook that automatically runs:
- `cargo fmt --check` - Ensures code is formatted
- `cargo clippy -- -D warnings` - Catches lint issues

The hook is located at `.git/hooks/pre-commit`. If you need to bypass it temporarily:
```bash
git commit --no-verify
```

### Documentation

- Update `AGENTS.md` if you change build commands, architecture, or common development tasks
- Update `CONTRIBUTING.md` if you change development workflow or coding guidelines
- Update crate-level documentation (`//!` comments) for public API changes

### PR Title Format

- `feat: add new feature` for new features
- `fix: resolve bug` for bug fixes
- `docs: update documentation` for documentation changes
- `refactor: improve code structure` for refactoring
- `test: add tests` for test additions
- `chore: update dependencies` for maintenance tasks

## Commit Messages

Follow the [Conventional Commits](https://www.conventionalcommits.org/) specification:

```
<type>(<scope>): <description>

[optional body]

[optional footer(s)]
```

### Types

| Type | Description |
|------|-------------|
| `feat` | New feature |
| `fix` | Bug fix |
| `docs` | Documentation only |
| `refactor` | Code change without fix/feature |
| `test` | Adding/updating tests |
| `chore` | Maintenance tasks |
| `perf` | Performance improvement |

### Examples

```
feat(encode): add AV1 codec support

Add support for AV1 encoding via NVENC and SVT-AV1 software encoder.
Includes configuration options and muxer updates.

Closes #123
```

```
fix(capture): handle DXGI access lost gracefully

When DXGI loses access (e.g., secure desktop), properly release
resources and attempt reacquisition instead of crashing.
```

## Architecture Overview

### Data Flow

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│   Capture   │────▶│   Encode    │────▶│   Buffer    │
│  (DXGI/     │     │  (NVENC/    │     │   (Ring)    │
│   WASAPI)   │     │   AMF/SW)   │     │             │
└─────────────┘     └─────────────┘     └──────┬──────┘
                                               │
                      ┌─────────────┐          │
                      │   Output    │◀─────────┘
                      │   (MP4)     │
                      └─────────────┘
```

### Key Components

| Component | Location | Responsibility |
|-----------|----------|----------------|
| `ReplayEngine` | `crates/liteclip-core/src/engine.rs` | Facade for embedding, wraps AppState |
| `AppState` | `crates/liteclip-core/src/app/state.rs` | Central state coordinator |
| `RecordingPipeline` | `crates/liteclip-core/src/app/pipeline/manager.rs` | Orchestrates capture → encode → buffer |
| `ReplayBuffer` | `crates/liteclip-core/src/buffer/ring/spmc_ring.rs` | Lock-free SPMC ring buffer |
| `DxgiCapture` | `crates/liteclip-core/src/capture/dxgi/` | DXGI Desktop Duplication screen capture |
| `AudioCapture` | `crates/liteclip-core/src/capture/audio/` | WASAPI audio capture (system + mic) |
| `Encoder` | `crates/liteclip-core/src/encode/` | Video encoding abstraction |
| `Config` | `crates/liteclip-core/src/config/config_mod/types.rs` | TOML configuration types |
| `PlatformHandle` | `src/platform/mod.rs` | Hotkeys, tray, notifications |
| `Gallery` | `src/gui/gallery.rs` | Clip browser and editor UI |

### Workspace Structure

This is a Cargo workspace with two crates:

| Crate | Path | Description |
|-------|------|-------------|
| `liteclip-replay` | `./` (root) | GUI application binary |
| `liteclip-core` | `crates/liteclip-core/` | Reusable engine library |

When working on the core engine, run tests from the workspace root:
```bash
cargo test -p liteclip-core
```

When working on the GUI application:
```bash
cargo test -p liteclip-replay
```

### Hardware encoders (NVENC / Intel QSV)

Maintainers may not have NVIDIA or Intel GPUs to exercise every path. **Pull requests** that change NVENC or QSV behavior should include **reporter verification**: GPU model, driver (or FFmpeg build) notes, and relevant `tracing` output.

**Authoritative checklist** (duplicate registry sites are called out in code comments there):

- Hub / overview: [`crates/liteclip-core/src/encode/ffmpeg/mod.rs`](crates/liteclip-core/src/encode/ffmpeg/mod.rs) (module-level `//!` docs)
- NVENC implementation: [`crates/liteclip-core/src/encode/ffmpeg/nvenc.rs`](crates/liteclip-core/src/encode/ffmpeg/nvenc.rs)
- QSV implementation: [`crates/liteclip-core/src/encode/ffmpeg/qsv.rs`](crates/liteclip-core/src/encode/ffmpeg/qsv.rs)
- AMF (reference path many contributors can run): [`crates/liteclip-core/src/encode/ffmpeg/amf.rs`](crates/liteclip-core/src/encode/ffmpeg/amf.rs)
- Encoder options: [`crates/liteclip-core/src/encode/ffmpeg/options.rs`](crates/liteclip-core/src/encode/ffmpeg/options.rs)
- Codec names + GPU transport: [`crates/liteclip-core/src/encode/encoder_mod/types.rs`](crates/liteclip-core/src/encode/encoder_mod/types.rs)
- Probe + auto-detect: [`crates/liteclip-core/src/encode/encoder_mod/functions.rs`](crates/liteclip-core/src/encode/encoder_mod/functions.rs)
- Config enum: [`crates/liteclip-core/src/config/config_mod/types.rs`](crates/liteclip-core/src/config/config_mod/types.rs) (`EncoderType`)
- Settings UI: [`src/gui/settings.rs`](src/gui/settings.rs)

**Manual test:** set the encoder explicitly (not Auto), record a short clip, and confirm logs do not show unexpected CPU fallback for GPU-capable setups.

**Gallery decode** uses generic D3D11VA in [`src/gui/gallery/decode_pipeline/mod.rs`](src/gui/gallery/decode_pipeline/mod.rs)—not per-vendor NVENC/QSV decode; encoding bugs usually belong under `encode/ffmpeg/`.

### Memory Management

The codebase uses `bytes::Bytes` for zero-copy packet handling. When cloning an `EncodedPacket`, only the reference count is bumped (O(1)), not the underlying video data.

**Key patterns:**
- `EncodedPacket` stores video/audio data in `Bytes` for cheap cloning
- `ReplayBuffer` stores packets without copying
- Snapshots clone packets via `Arc` reference counting
- Use `Bytes::copy_from_slice()` only when you need an independent copy

### Error Recovery

The pipeline monitors health via `enforce_pipeline_health()`:
- Capture errors (DXGI access lost) trigger reacquisition
- Encoder errors propagate to the main loop
- Fatal errors invoke `CoreHost::on_pipeline_fatal()` if registered

When adding new components, ensure errors are propagated correctly through the health check system.

### Threading Model

```
Main Thread (Tokio async runtime)
├── Event Loop (tokio::select!)
│   ├── Platform events (hotkeys, tray)
│   ├── Health monitoring via enforce_pipeline_health()
│   └── Config I/O
│
├── Platform Thread (dedicated)
│   ├── Windows message loop
│   ├── Hotkey handling
│   └── Tray icon management
│
├── Capture Thread (spawned by pipeline)
│   ├── DXGI frame acquisition
│   └── Audio capture
│
├── Encode Thread (spawned by pipeline)
│   └── Video/audio encoding
│
└── Buffer (lock-free SPMC)
    └── Single producer (encode), Multiple consumers (save)
```

**Thread coordination:**
- Pipeline threads (capture, encode) are spawned by `RecordingPipeline` in `crates/liteclip-core/src/app/pipeline/manager.rs`
- The main thread uses `tokio::task::spawn_blocking` for blocking operations on `AppState`
- The buffer is SPMC: single encoder producer, multiple clip-save consumers

### Error Handling

- Use `anyhow::Result` for fallible operations
- Use `thiserror` for custom error types
- Propagate errors to `AppState::enforce_pipeline_health()` for recovery
- Log errors with `tracing`

## Debugging

### Enable Verbose Logging

```powershell
# PowerShell
$env:RUST_LOG = "debug,liteclip_core=trace,wgpu=warn,naga=warn"
cargo run
```

### Common Issues

**DXGI_ACCESS_LOST errors:** Expected when the desktop switches (UAC, lock screen, secure desktop). The capture thread handles this by releasing and reacquiring. Don't panic on access lost.

**Hardware encoder fallback:** Check logs for "unexpected CPU fallback" messages. Hardware encoders (NVENC/AMF/QSV) fall back to software when unavailable.

**FFmpeg DLL not found:** Ensure FFmpeg 6.0+ shared DLLs are next to the executable or on PATH. Required: `avcodec-*.dll`, `avformat-*.dll`, `avutil-*.dll`, `swscale-*.dll`, `swresample-*.dll`, `avfilter-*.dll`.

**Frame drops:** If the encode thread can't keep up, `BackpressureState` signals capture to drop frames. Check encoder performance with verbose logging.

### Memory Profiling

The codebase includes memory diagnostics. Use:
- Windows Performance Recorder for heap analysis
- Visual Studio Diagnostic Tools for memory snapshots
- Check `crates/liteclip-core/src/memory_diag.rs` for utilities

## Questions?

Open an issue for questions or discussion about contributions.