# AGENTS.md

This file provides guidance to agents when working with code in this repository.

## Project Context

LiteClip Recorder is a Windows-only screen capture application using D3D11/DXGI Desktop Duplication. It captures desktop frames, encodes them using hardware encoders (NVENC/AMF/QSV) via FFmpeg CLI or software JPEG encoding, stores them in a memory-bounded ring buffer, and saves clips on hotkey trigger.

## Build Commands

Standard Cargo commands work. The `ffmpeg` feature flag enables FFmpeg code paths but requires FFmpeg at runtime (not link-time):

```powershell
# Build
cargo build --release --features ffmpeg

# Run
cargo run --features ffmpeg

# Check (faster than build)
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
- Prefer `Bytes` from `bytes` crate for ref-counted buffers

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

### Documentation
- Use `//!` module-level docs at top of file
- Use `///` for public items
- Document panics, safety invariants, and examples

### Unsafe Code
- Minimize unsafe blocks
- Document safety invariants with `// SAFETY:` comments
- Prefer `windows` crate safe wrappers where possible

## Module Structure

The codebase has been split using `splitrs` for better maintainability. Large files (>500 lines) are now organized into submodules:

### Split Modules

| Module | Original File | New Structure |
|--------|---------------|---------------|
| **clip/muxer** | `src/clip/muxer.rs` (1268 lines) | `types.rs` + `functions.rs` |
| **encode/hw_encoder** | `src/encode/hw_encoder.rs` (1152 lines) | `types.rs`, `functions.rs`, `amfencoder_traits.rs`, `nvencencoder_traits.rs`, `qsvencoder_traits.rs`, `hardwareencoderbase_traits.rs` |
| **encode/encoder_mod** | `src/encode/mod.rs` (682 lines) | `types.rs`, `functions.rs`, `functions_2.rs`, `encodedpacket_traits.rs`, `encoderconfig_traits.rs` |
| **capture/dxgi** | `src/capture/dxgi.rs` (577 lines) | `types.rs`, `functions.rs`, `dxgicapture_traits.rs` |
| **config/config_mod** | `src/config.rs` (512 lines) | `types.rs`, `functions.rs`, 5 trait files (audio, video, general, advanced, hotkey config traits) |
| **buffer/ring** | `src/buffer/ring.rs` (502 lines) | `types.rs`, `functions.rs`, `sharedreplaybuffer_traits.rs` |

### Import Patterns After Split

When working with split modules:
- Public items are re-exported via `mod.rs` using `pub use types::*` and `pub use functions::*`
- Trait implementations are in separate `*_traits.rs` files
- Default helper functions (for serde) are in `functions.rs` and marked `pub` or `pub(super)`
- Cross-module access within a split directory uses `super::types::TypeName` or `super::functions::function_name`

### Working with Split Files

```rust
// Example: importing from split hw_encoder module
use crate::encode::hw_encoder::{HardwareEncoderBase, NvencEncoder, AmfEncoder, QsvEncoder};

// The types are re-exported from submodules
use crate::encode::hw_encoder::types::HardwareEncoderBase;
use crate::encode::hw_encoder::functions::resolve_ffmpeg_command;
```

## Critical Code Patterns

### Hardware Encoder Selection
Encoder selection happens in [`encode/mod.rs`](src/encode/mod.rs) but actual FFmpeg command building with encoder-specific flags is in [`encode/hw_encoder.rs`](src/encode/hw_encoder.rs). Each encoder requires different flags:

- **h264_nvenc**: Uses `preset=p4`, `tune=ll` (low latency), `rc=vbr`, `cq=23`
- **h264_amf`: **CRITICAL** - requires `-bf 0` (disable B-frames), `-sei +aud`, `-vsync cfr`. Missing B-frame disable produces unplayable output.
- **h264_qsv**: Uses `preset=veryfast`

Encoder initialization is lazy - happens on first frame, not at encoder creation.

### Frame Data Flow
[`CapturedFrame`](src/capture/mod.rs) contains **both** a D3D11 texture handle AND CPU BGRA bytes. Even when using hardware encoding that only needs the GPU texture, CPU readback happens unconditionally (Phase 1 limitation). This means unnecessary memory copies.

### Ring Buffer Eviction
The [`ReplayBuffer`](src/buffer/ring.rs) evicts based on **duration**, with memory cap as a safety guard. The `duration` field controls how long of a rolling window is kept. The memory cap only kicks in if timestamps are invalid or the configured duration would exceed available memory.

### Error Handling
Many error paths log with `warn!`/`error!` and continue silently rather than propagating. The encoder thread may be dead but the application continues running. Always check thread join results.

### Configuration
Config stored at `%APPDATA%/liteclip-replay/liteclip-replay.toml`. Resolution config is ignored if `use_native_resolution` is true - actual resolution comes from the first captured frame.

## Testing

Hardware encoder tests require FFmpeg and compatible GPU. Most unit tests can run without FFmpeg:

```powershell
# Run tests without FFmpeg feature
cargo test
```

## Dependencies

- `windows` crate with many Win32 features enabled - see Cargo.toml for full list
- `tokio` for async runtime
- `crossbeam` for thread channels
- `parking_lot` for synchronization primitives
- `bytes` for ref-counted buffers (used throughout for cheap cloning)
