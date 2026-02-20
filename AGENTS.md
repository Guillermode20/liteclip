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

## Codebase Structure

### Root Files
- **`Cargo.toml`** - Project configuration with dependencies, features (ffmpeg flag), and release profile settings (LTO, panic=abort)
- **`Cargo.lock`** - Dependency lock file for reproducible builds
- **`build.rs`** - Build script for conditional compilation
- **`AGENTS.md`** - This file: AI agent guidance and project documentation
- **`README.md`** - Project readme (if present)
- **`.gitignore`** - Git ignore patterns
- **`LICENSE`** - Project license (if present)

### Build & Distribution
- **`installer/`** - WiX Toolset Windows installer configuration
  - **`Components.wxs`** - Installer components definition
  - **`Directories.wxs`** - Installation directory structure
  - **`Features.wxs`** - Installer features
  - **`Product.wxs`** - Main product definition
  - **`UI.wxs`** - Installer user interface
  - **`Variables.wxs`** - Installer variables
  - **`Shortcuts.wxs`** - Start menu shortcuts
  - **`Registry.wxs`** - Windows registry entries
  - **`Dialogs/`** - Custom installer dialogs
  - **`output/`** - Generated installer files (.msi, .wixpdb, .cab)
- **`ffmpeg/`** - Bundled FFmpeg binary directory
  - **`bin/ffmpeg.exe`** - FFmpeg executable for encoding

### Documentation & Planning
- **`plans/`** - Architecture and feature planning documents
  - **`architecture-review.md`** - Architecture review notes
  - **`tray-integration-architecture.md`** - System tray integration design
- **`.kilocode/`** - Kilocode IDE rules and configurations
  - **`rules/`** - Development rules and guidelines
- **`.opencode/`** - OpenCode IDE plans and configurations
  - **`plans/`** - Development plans and milestones

### Source Code (`src/`)

#### Core Application
- **`main.rs`** - Application entry point with system tray integration, event loop, hotkey handling, and graceful shutdown
- **`lib.rs`** - Library root with module declarations, type aliases (AppHandle, Result, Error), and trait implementations
- **`app.rs`** - Core application state management (AppState, RecordingPipeline, ClipManager) and lifecycle coordination

#### Platform Layer (`src/platform/`)
- **`mod.rs`** - Platform abstraction with event types (AppEvent, TrayEvent, HotkeyAction), PlatformHandle for thread management
- **`msg_loop.rs`** - Windows message loop thread with hidden HWND for hotkey registration and system tray
- **`hotkeys.rs`** - Global hotkey registration and handling using Windows RegisterHotKey API
- **`tray.rs`** - System tray icon, menu, and notification management

#### Capture Subsystem (`src/capture/`)
- **`mod.rs`** - Capture module exports and common types (CapturedFrame, CaptureBackend, CaptureConfig)
- **`dxgi/`** - DXGI Desktop Duplication capture implementation
  - **`types.rs`** - DxgiCapture struct and related types
  - **`functions.rs`** - DXGI capture initialization, frame acquisition, and cleanup
  - **`dxgicapture_traits.rs`** - DxgiCapture trait implementations
- **`audio/`** - WASAPI audio capture implementation
  - **`mod.rs`** - Audio module exports and WasapiAudioManager
  - **`manager.rs`** - Audio capture coordination (system + microphone)
  - **`system.rs`** - System audio capture using WASAPI loopback
  - **`mic.rs`** - Microphone audio capture
  - **`mixer.rs`** - Audio stream mixing and routing
  - **`device_info.rs`** - Audio device enumeration and capabilities
- **`backpressure.rs`** - Capture backpressure management and flow control

#### Encoding Subsystem (`src/encode/`)
- **`mod.rs`** - Encoder module exports and spawning functions
- **`hw_encoder/`** - Hardware encoder implementations via FFmpeg CLI
  - **`types.rs`** - Hardware encoder types (HardwareEncoderBase, NvencEncoder, AmfEncoder, QsvEncoder)
  - **`functions.rs`** - FFmpeg command building, encoder spawning, and management
  - **`managedffmpegprocess_traits.rs`** - ManagedFfmpegProcess lifecycle management
  - **`hardwareencoderbase_traits.rs`** - Base hardware encoder trait
  - **`nvencencoder_traits.rs`** - NVIDIA NVENC encoder implementation
  - **`amfencoder_traits.rs`** - AMD AMF encoder implementation
  - **`qsvencoder_traits.rs`** - Intel QSV encoder implementation
- **`encoder_mod/`** - Encoder configuration and packet handling
  - **`types.rs`** - Encoder configuration types (EncoderConfig, EncoderHandle)
  - **`functions.rs`** - Encoder spawning and management functions
  - **`functions_2.rs`** - Additional encoder utility functions
  - **`encoderconfig_traits.rs`** - Encoder configuration trait implementations
  - **`encodedpacket_traits.rs`** - Encoded packet handling traits
- **`sw_encoder.rs`** - Software JPEG encoder implementation
- **`cpu_readback.rs`** - CPU readback mode for hardware encoders
- **`frame_writer.rs`** - Frame writing utilities

#### Buffer Management (`src/buffer/`)
- **`mod.rs`** - Buffer module exports
- **`ring/`** - Ring buffer implementation for replay storage
  - **`types.rs`** - ReplayBuffer types and data structures
  - **`functions.rs`** - Ring buffer operations (push, evict, snapshot)
  - **`sharedreplaybuffer_traits.rs`** - SharedReplayBuffer trait implementations

#### Clip Management (`src/clip/`)
- **`mod.rs`** - Clip module exports
- **`muxer/`** - FFmpeg-based clip muxing and saving
  - **`types.rs`** - Muxer configuration and data structures
  - **`functions.rs`** - Clip saving, muxing, and file operations

#### Configuration (`src/config/`)
- **`mod.rs`** - Configuration module exports and loading functions
- **`config_mod/`** - Configuration types and validation
  - **`types.rs`** - Main Config struct and all configuration types
  - **`functions.rs`** - Configuration loading, saving, and validation
  - **`generalconfig_traits.rs`** - General configuration traits
  - **`videoconfig_traits.rs`** - Video configuration traits
  - **`audioconfig_traits.rs`** - Audio configuration traits
  - **`advancedconfig_traits.rs`** - Advanced configuration traits
  - **`hotkeyconfig_traits.rs`** - Hotkey configuration traits

#### Utilities
- **`d3d.rs`** - D3D11 device and context management
- **`metrics.rs`** - Performance metrics and monitoring

## AI Agent Best Practices

1. **Fast Verification**: ALWAYS use `cargo check --features ffmpeg` instead of `cargo build` to quickly verify compile errors.
2. **Finding Files**: Use `code_search` or `find_by_name` when looking for implementations, as the `splitrs` pattern moves logic from `mod.rs` into `types.rs` and `functions.rs`.
3. **Writing Edits**: When editing split modules, make sure you're editing `types.rs` for structs/enums and `functions.rs` for implementations.
4. **Zero-Copy Rule**: When working with frames or packets, avoid `Vec<u8>` cloning. Use `Bytes::clone()` (which only increments a ref count) or `BytesMut::split()`.
5. **No Blind Panics**: Always return `anyhow::Result` from worker threads and check the thread `.join()` result in Drop or Stop methods.
