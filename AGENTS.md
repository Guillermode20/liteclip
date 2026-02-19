# AGENTS.md

This file provides guidance to AI agents when working with code in this repository.

## Project Context

LiteClip Replay is a Windows-only screen capture application using D3D11/DXGI Desktop Duplication or FFmpeg hardware pull modes. It captures desktop frames, encodes them using hardware encoders (NVENC/AMF/QSV) via FFmpeg CLI or software JPEG encoding, stores them in a memory-bounded ring buffer, and saves clips on hotkey trigger.

## Build Commands

Standard Cargo commands work. The `ffmpeg` feature flag enables FFmpeg code paths but requires FFmpeg at runtime (not link-time):

```powershell
# Build
cargo build --release --features ffmpeg

# Run
cargo run --features ffmpeg

# Check (faster than build - ALWAYS prefer this for validation)
cargo check --features ffmpeg
```

**Critical:** Release profile uses `lto = "fat"` and `panic = "abort"` - expect longer compile times for optimized binaries.

## Test Commands

```powershell
# Run all tests
cargo test --features ffmpeg

# Run a single test
cargo test --features ffmpeg test_frame_duration

# Run tests for a specific module
cargo test --features ffmpeg encode::

# Run tests with output
cargo test --features ffmpeg -- --nocapture
```

## Lint Commands

```powershell
# Run Clippy (no warnings allowed)
cargo clippy --features ffmpeg -- -D warnings

# Auto-fix Clippy warnings
cargo clippy --features ffmpeg --fix --allow-dirty

# Format code
cargo fmt

# Check formatting without changes
cargo fmt -- --check
```

## Platform Requirements

- Windows 10+ with DXGI 1.2 support
- Windows SDK (for D3D11 headers)
- FFmpeg in PATH or at expected locations (checked in order: `LITECLIP_FFMPEG_PATH` env var, `./ffmpeg/bin/ffmpeg.exe`, `<exe_dir>/ffmpeg/bin/ffmpeg.exe`, system PATH)

## Code Style Guidelines

### Imports
- Group imports: `std`, external crates, then internal (`crate::`)
- Use `use anyhow::Result;` for error handling
- Use `use tracing::{debug, error, info, warn};` for logging
- Prefer `Bytes` or `BytesMut` from `bytes` crate for zero-copy ref-counted buffers.

### Types
- Use `i64` for QPC timestamps (10MHz units)
- Use `u32` for dimensions (width, height)
- Use `Duration` for time spans
- Prefer `parking_lot::RwLock` over `std::sync::RwLock`

### Naming Conventions
- `PascalCase` for types, traits, enums
- `snake_case` for functions, variables, modules
- `SCREAMING_SNAKE_CASE` for constants
- Suffix config types with `Config` (e.g., `EncoderConfig`)
- Suffix handle types with `Handle` (e.g., `EncoderHandle`)

### Error Handling
- Use `anyhow::Result` for most functions
- Use `thiserror` for custom error enums when needed
- Log recoverable errors with `warn!`/`error!` and continue
- Validate config values in `Config::validate()` - clamp to safe ranges

### Unsafe Code
- Minimize unsafe blocks
- Document safety invariants with `// SAFETY:` comments
- Prefer `windows` crate safe wrappers where possible

## Module Structure

The codebase has been split using `splitrs` for better maintainability. Large files (>500 lines) are organized into submodules:

| Module | New Structure |
|--------|---------------|
| **clip/muxer** | `types.rs` + `functions.rs` |
| **encode/hw_encoder** | `types.rs`, `functions.rs`, `*encoder_traits.rs` |
| **encode/encoder_mod** | `types.rs`, `functions.rs`, `functions_2.rs`, `*_traits.rs` |
| **capture/dxgi** | `types.rs`, `functions.rs`, `dxgicapture_traits.rs` |
| **config/config_mod** | `types.rs`, `functions.rs`, 5 trait files |
| **buffer/ring** | `types.rs`, `functions.rs`, `sharedreplaybuffer_traits.rs` |

### Import Patterns After Split
- Public items are re-exported via `mod.rs` using `pub use types::*` and `pub use functions::*`
- Trait implementations are in separate `*_traits.rs` files
- Cross-module access within a split directory uses `super::types::TypeName` or `super::functions::function_name`

## Critical Code Patterns & Architecture

### Core Pipeline Managers
- **`AppState`** (`src/app.rs`): Lightweight coordinator holding Config, Buffer, and the Pipeline.
- **`RecordingPipeline`** (`src/app.rs`): Encapsulates capture and encoder orchestration, audio routing, and recording lifecycle state machine.
- **`ClipManager`** (`src/app.rs`): Handles taking snapshots of the buffer and spawning the async muxer task.

### Hardware Encoder Selection & Modes
- **Hardware Pull Mode**: If hardware encoding (NVENC/AMF/QSV) is explicitly selected and `use_cpu_readback` is false, the application bypasses `DxgiCapture` and uses FFmpeg's `ddagrab` input directly.
- **CPU Readback Mode**: Uses DXGI Desktop Duplication to copy frames to CPU memory (`BytesMut`), which are then piped into FFmpeg via stdin.
- **FFmpeg Lifecycle**: Managed by `ManagedFfmpegProcess` to guarantee robust shutdown and prevent zombie FFmpeg processes upon app exit or restart.

### Frame Data Flow
`CapturedFrame` (`src/capture/mod.rs`) uses `bytes::Bytes` for CPU memory. The DXGI capture thread splits and freezes chunks from a pre-allocated `BytesMut` pool to achieve zero-copy cloning when pushing to channels.

### Ring Buffer Eviction
The `ReplayBuffer` (`src/buffer/ring/types.rs`) uses an `O(1)` eviction strategy. It maintains a `VecDeque<(i64, usize)>` for its keyframe index, avoiding O(N) tree rebuilds. It evicts strictly based on duration, using memory limit as a fallback safety guard.

### A/V Sync
Audio and Video timestamps use identical QPC (QueryPerformanceCounter) timelines. The `WasapiAudioManager` forwards both system and mic audio. The Muxer uses relative PTS alignment and silence insertion (`qpc_delta_to_aligned_pcm_bytes`) to ensure audio perfectly aligns with video without drift.

## AI Agent Best Practices

1. **Fast Verification**: ALWAYS use `cargo check --features ffmpeg` instead of `cargo build` to quickly verify compile errors.
2. **Finding Files**: Use `code_search` or `find_by_name` when looking for implementations, as the `splitrs` pattern moves logic from `mod.rs` into `types.rs` and `functions.rs`.
3. **Writing Edits**: When editing split modules, make sure you're editing `types.rs` for structs/enums and `functions.rs` for implementations.
4. **Zero-Copy Rule**: When working with frames or packets, avoid `Vec<u8>` cloning. Use `Bytes::clone()` (which only increments a ref count) or `BytesMut::split()`.
5. **No Blind Panics**: Always return `anyhow::Result` from worker threads and check the thread `.join()` result in Drop or Stop methods.
