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

1. **Fork the repository** and create your branch from `main`
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
| `AppState` | `app/state.rs` | Central state coordinator |
| `RecordingPipeline` | `app/pipeline/` | Orchestrates capture → encode → buffer |
| `ReplayBuffer` | `buffer/ring/` | Lock-free ring buffer for replay storage |
| `DxgiCapture` | `capture/dxgi/` | DXGI Desktop Duplication screen capture |
| `AudioCapture` | `capture/audio/` | WASAPI audio capture |
| `Encoder` | `encode/` | Video encoding abstraction |
| `PlatformHandle` | `platform/` | Hotkeys, tray, notifications |

### Hardware encoders (NVENC / Intel QSV)

Maintainers may not have NVIDIA or Intel GPUs to exercise every path. **Pull requests** that change NVENC or QSV behavior should include **reporter verification**: GPU model, driver (or FFmpeg build) notes, and relevant `tracing` output.

**Authoritative checklist** (duplicate registry sites are called out in code comments there):

- Hub / overview: [`src/encode/ffmpeg/mod.rs`](src/encode/ffmpeg/mod.rs) (module-level `//!` docs)
- NVENC implementation: [`src/encode/ffmpeg/nvenc.rs`](src/encode/ffmpeg/nvenc.rs)
- QSV implementation: [`src/encode/ffmpeg/qsv.rs`](src/encode/ffmpeg/qsv.rs)
- AMF (reference path many contributors can run): [`src/encode/ffmpeg/amf.rs`](src/encode/ffmpeg/amf.rs)
- Encoder options: [`src/encode/ffmpeg/options.rs`](src/encode/ffmpeg/options.rs)
- Codec names + GPU transport: [`src/encode/encoder_mod/types.rs`](src/encode/encoder_mod/types.rs)
- Probe + auto-detect: [`src/encode/encoder_mod/functions.rs`](src/encode/encoder_mod/functions.rs)
- Config enum: [`src/config/config_mod/types.rs`](src/config/config_mod/types.rs) (`EncoderType`)
- Settings UI: [`src/gui/settings.rs`](src/gui/settings.rs)

**Manual test:** set the encoder explicitly (not Auto), record a short clip, and confirm logs do not show unexpected CPU fallback for GPU-capable setups.

**Gallery decode** uses generic D3D11VA in [`src/gui/gallery/decode_pipeline.rs`](src/gui/gallery/decode_pipeline.rs)—not per-vendor NVENC/QSV decode; encoding bugs usually belong under `encode/ffmpeg/`.

### Threading Model

```
Main Thread (async runtime)
├── Event Loop (tokio::select!)
│   ├── Platform events (hotkeys, tray)
│   └── Health monitoring
│
├── Platform Thread
│   ├── Windows message loop
│   ├── Hotkey handling
│   └── Tray icon management
│
├── Capture Thread
│   ├── DXGI frame acquisition
│   └── Audio capture
│
├── Encode Thread
│   └── Video/audio encoding
│
└── Buffer (lock-free)
    └── SPMC: Single producer (encode), Multiple consumers (save)
```

### Error Handling

- Use `anyhow::Result` for fallible operations
- Use `thiserror` for custom error types
- Propagate errors to `AppState::enforce_pipeline_health()` for recovery
- Log errors with `tracing`

## Questions?

Open an issue for questions or discussion about contributions.