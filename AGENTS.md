# AGENTS.md

Essential guidelines for autonomous agents working on LiteClip — a native Windows screen recorder built in Rust.

## Quick Commands

| Task | Command |
|------|---------|
| Build (debug) | `cargo build` |
| Build (release) | `cargo build --release --features ffmpeg` |
| Run | `cargo run` |
| Fast check | `cargo check` |
| **Test all** | `cargo test` |
| **Test single** | `cargo test test_name` |
| **Test with output** | `cargo test -- --nocapture` |
| **Format check** | `cargo fmt --check` |
| **Lint** | `cargo clippy -- -D warnings` |

## Code Style

### Import Order
```rust
// 1. Standard library
use std::sync::{Arc, atomic::AtomicBool};

// 2. External crates
use anyhow::{Context, Result, bail};
use bytes::Bytes;
use tracing::info;

// 3. Internal crate
use crate::config::Config;
use super::SomeType;
```

### Naming Conventions
| Category | Convention | Example |
|----------|------------|---------|
| Types (structs, enums) | PascalCase | `EncoderType`, `DxgiCapture` |
| Functions, methods | snake_case | `start_recording()`, `save_clip()` |
| Variables | snake_case | `frame_count`, `config_path` |
| Constants | UPPER_SNAKE_CASE | `MAX_MEMORY_BYTES`, `LOG_INTERVAL` |
| Type aliases | PascalCase | `pub type Result<T> = std::result::Result<T, LiteClipError>;` |

### Error Handling
- Use `anyhow` for error propagation: `fn foo() -> Result<T>`
- Attach context: `.context("Failed to create device")?`
- Early returns: `bail!("DXGI access lost")`
- Custom error type for API boundaries: `LiteClipError`

### Documentation
- Module docs: `//! Description`
- Item docs: `/// Description`
- Include sections: `# Arguments`, `# Errors`, `# Returns`, `# Example`
- Mark unsafe: `# Safety` docs required for `unsafe` blocks

### Type Patterns
```rust
// Result aliases
pub type Result<T, E = LiteClipError> = std::result::Result<T, E>;

// Zero-copy with Bytes
pub struct EncodedPacket {
    data: Bytes,  // Cheap clone (ref count bump)
    pts: i64,
    keyframe: bool,
}

// Explicit atomic ordering
use std::sync::atomic::{AtomicUsize, Ordering};
counter.fetch_add(1, Ordering::Relaxed);
```

### Common Attributes
```rust
#[must_use]                              // Important return values
#[cfg(windows)]                          // Windows-only code
#[serde(rename_all = "snake_case")]      // Config serialization
#[allow(clippy::too_many_lines)]         // When needed
```

## Project Structure

```
liteclip-recorder/
├── src/                          # Main GUI application
│   ├── main.rs                   # Entry point
│   ├── gui/                      # egui UI (settings, gallery)
│   ├── platform/                 # Windows hotkeys, tray
│   └── detection/                # Game detection
│
├── crates/liteclip-core/         # Reusable engine library
│   ├── src/
│   │   ├── engine.rs             # ReplayEngine facade
│   │   ├── app/                  # State, pipeline, clips
│   │   ├── capture/              # DXGI + WASAPI audio
│   │   ├── encode/               # NVENC/AMF/QSV/Software
│   │   ├── buffer/ring/          # Lock-free SPMC ring buffer
│   │   ├── output/               # MP4 muxing, clip saving
│   │   └── config/               # TOML configuration
│   └── tests/                    # Integration tests
│
├── installer/                    # WiX MSI installer
└── Cargo.toml                    # Workspace root
```

## Testing

```bash
# Run all tests
cargo test

# Run specific test by name
cargo test test_snapshot_cheap_clone

# Run tests in specific file
cargo test --test config_roundtrip

# Run with println! output visible
cargo test -- --nocapture

# Compile tests without running
cargo test --no-run

# Run examples (require ffmpeg feature)
cargo run --example minimal_engine --features ffmpeg
```

Tests are inline (`#[cfg(test)]` modules) or in `crates/liteclip-core/tests/`.

## Critical Gotchas

### FFmpeg DLL Requirements
- FFmpeg 6.0+ shared DLLs must be next to the executable
- Required: `avcodec-61.dll`, `avformat-61.dll`, `avutil-59.dll`, `swresample-5.dll`, `swscale-8.dll`
- Build script copies from `ffmpeg_dev/sdk/bin` automatically

### DXGI Access Lost
- `DXGI_ERROR_ACCESS_LOST` is expected on secure desktop (UAC, lock screen)
- Capture thread handles reacquisition — **do not panic**
- Code releases duplication, waits, then retries

### Hardware Encoder Fallback
- NVENC/AMF/QSV automatically fall back to software encoding if unavailable
- Check logs for unexpected CPU fallback messages
- Encoder auto-detection in `encoder_mod/functions.rs`

### Windows-Only
- This is a Windows-only codebase
- All platform code gated with `#[cfg(windows)]`
- Uses windows-rs for Win32 APIs

### Hardware Encoder Testing Gap
- **AMD AMF** is the only actively tested hardware encoder (maintainer has AMD GPU)
- **NVIDIA NVENC** and **Intel QSV** paths have **never been tested on real hardware**
- Code is written to spec based on FFmpeg documentation but may contain bugs that only surface on actual NVIDIA/Intel GPUs
- When modifying encoder code, keep all three vendor paths consistent
- Contributors with NVIDIA/Intel GPUs should test before merge — see CONTRIBUTING.md for checklists
- Verification tests in `crates/liteclip-core/tests/hardware_encoder_verification.rs` validate config/metadata but cannot verify actual encoding without real hardware

### Zero-Copy Memory
- Use `bytes::Bytes` for cheap cloning (O(1) ref count bump)
- Ring buffer proactively evicts at 80% memory watermark
- Max 512MB outstanding for concurrent snapshots

## Commit Format

Use conventional commits: `type(scope): description`

Examples:
- `fix(capture): handle DXGI access lost gracefully`
- `feat(encode): add NVENC D3D11 shared device support`
- `refactor(buffer): simplify eviction logic`

## Logging for Debug

```powershell
$env:RUST_LOG = "debug,liteclip_core=trace"
cargo run
```
