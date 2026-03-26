# AGENTS.md

Essential information for autonomous agents working on LiteClip Replay.

## Project Overview

LiteClip Replay is a native Windows desktop screen recorder built in Rust. It continuously records in the background using a replay buffer and lets users save clips on demand. The architecture separates the core engine (reusable library) from the GUI application.

## Quick Reference

| Task | Command |
|------|---------|
| Build (debug) | `cargo build` |
| Build (release) | `cargo build --release --features ffmpeg` |
| Run | `cargo run` |
| Test | `cargo test` |
| Format check | `cargo fmt --check` |
| Lint | `cargo clippy -- -D warnings` |
| Fast validation | `cargo check` |

## Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              DATA FLOW                                       │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│   ┌──────────────┐    ┌──────────────┐    ┌──────────────┐    ┌──────────┐ │
│   │   Capture    │───▶│    Encode    │───▶│    Buffer    │───▶│  Output  │ │
│   │  DXGI/       │    │  NVENC/AMF/  │    │   (Ring)     │    │  (MP4)   │ │
│   │  WASAPI      │    │  QSV/SW      │    │   SPMC       │    │          │ │
│   └──────────────┘    └──────────────┘    └──────────────┘    └──────────┘ │
│         │                    │                    │                         │
│         v                    v                    v                         │
│   crates/liteclip-core/src/capture    buffer/ring    output/saver.rs       │
│                                        spmc_ring.rs                         │
└─────────────────────────────────────────────────────────────────────────────┘
```

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
│   ├── DXGI Desktop Duplication
│   └── WASAPI audio capture
│
├── Encode Thread (spawned by pipeline)
│   └── Video/audio encoding
│
└── Buffer (lock-free SPMC)
    └── Single producer (encode), Multiple consumers (save clips)
```

### Key Components

| Component | Path | Responsibility |
|-----------|------|----------------|
| `ReplayEngine` | `crates/liteclip-core/src/engine.rs` | Facade over AppState, main entry for embedders |
| `AppState` | `crates/liteclip-core/src/app/state.rs` | Central state coordinator |
| `RecordingPipeline` | `crates/liteclip-core/src/app/pipeline/manager.rs` | Orchestrates capture → encode → buffer |
| `LockFreeReplayBuffer` | `crates/liteclip-core/src/buffer/ring/spmc_ring.rs` | SPMC ring buffer with proactive eviction |
| `DxgiCapture` | `crates/liteclip-core/src/capture/dxgi/capture.rs` | DXGI Desktop Duplication |
| `AudioCapture` | `crates/liteclip-core/src/capture/audio/` | WASAPI audio (system + mic) |
| `FfmpegEncoder` | `crates/liteclip-core/src/encode/ffmpeg/mod.rs` | Video encoding abstraction |
| `Config` | `crates/liteclip-core/src/config/config_mod/types.rs` | TOML configuration types |
| `PlatformHandle` | `src/platform/mod.rs` | Hotkeys, tray, autostart |
| `Gallery` | `src/gui/gallery.rs` | Clip browser and editor UI |

## Project Structure

```
liteclip-recorder/
├── src/                          # Main application (GUI binary)
│   ├── main.rs                   # Entry point, event loop, initialization
│   ├── lib.rs                    # Library root for app types
│   ├── gui/
│   │   ├── manager.rs            # egui app wrapper
│   │   ├── settings.rs           # Settings panel UI
│   │   ├── gallery.rs            # Clip browser/editor
│   │   └── gallery/
│   │       ├── decode_pipeline/  # Video decode for preview
│   │       └── editor_panels/    # Trim UI, timeline
│   ├── platform/
│   │   ├── mod.rs                # Platform thread, PlatformHandle
│   │   ├── hotkeys.rs            # Global hotkey registration
│   │   ├── tray.rs               # System tray icon
│   │   ├── autostart.rs          # Windows startup registry
│   │   └── msg_loop.rs           # Windows message pump
│   └── detection/
│       └── game.rs               # Detect running game from foreground window
│
├── crates/liteclip-core/         # Reusable engine library
│   ├── src/
│   │   ├── lib.rs                # Crate root, public API
│   │   ├── engine.rs             # ReplayEngine facade
│   │   ├── host.rs               # CoreHost trait for callbacks
│   │   ├── paths.rs              # AppDirs for config/paths
│   │   ├── error.rs              # LiteClipError enum
│   │   ├── app/
│   │   │   ├── state.rs          # AppState, health checking
│   │   │   ├── clip.rs           # ClipManager
│   │   │   └── pipeline/
│   │   │       ├── manager.rs    # RecordingPipeline lifecycle
│   │   │       ├── video.rs      # Video capture thread
│   │   │       └── audio.rs      # Audio capture thread
│   │   ├── capture/
│   │   │   ├── mod.rs            # CaptureConfig, CapturedFrame
│   │   │   ├── dxgi/             # DXGI Desktop Duplication
│   │   │   │   ├── capture.rs    # Main capture loop
│   │   │   │   ├── texture.rs    # NV12/BGRA texture pools
│   │   │   │   └── device.rs     # D3D11 device setup
│   │   │   ├── audio/            # WASAPI audio capture
│   │   │   │   ├── system.rs     # System audio (loopback)
│   │   │   │   ├── mic.rs        # Microphone with RNNoise
│   │   │   │   └── mixer.rs      # Audio stream mixing
│   │   │   └── backpressure.rs   # Frame drop signaling
│   │   ├── encode/
│   │   │   ├── mod.rs            # Encoder trait
│   │   │   ├── sw_encoder.rs     # Software encoder (libx264/libx265)
│   │   │   └── ffmpeg/
│   │   │       ├── mod.rs        # FfmpegEncoder main
│   │   │       ├── nvenc.rs      # NVIDIA NVENC
│   │   │       ├── amf.rs        # AMD AMF
│   │   │       ├── qsv.rs        # Intel Quick Sync
│   │   │       ├── software.rs   # FFmpeg software encode
│   │   │       └── options.rs    # Encoder option builder
│   │   ├── buffer/
│   │   │   ├── mod.rs            # Buffer trait
│   │   │   └── ring/
│   │   │       ├── spmc_ring.rs  # LockFreeReplayBuffer
│   │   │       └── types.rs      # BufferStats
│   │   ├── output/
│   │   │   ├── mod.rs            # Output types
│   │   │   ├── saver.rs          # spawn_clip_saver
│   │   │   ├── mp4.rs            # FfmpegMuxer
│   │   │   ├── sdk_ffmpeg_output.rs  # SDK-based export
│   │   │   └── sdk_export.rs     # Clip export with trimming
│   │   └── config/
│   │       ├── mod.rs            # Config loading
│   │       └── config_mod/
│   │           ├── types.rs      # Config structs, enums
│   │           └── functions.rs  # Defaults, validation
│   ├── tests/                    # Integration tests
│   └── examples/                 # Embedder examples
│
├── installer/                    # WiX MSI installer
├── .github/workflows/release.yml # CI/CD
└── Cargo.toml                    # Workspace root
```

## Memory Management Patterns

### Zero-Copy with `Bytes`

The codebase uses `bytes::Bytes` extensively for zero-copy packet handling:

```rust
// EncodedPacket uses Bytes for cheap ref-counting
pub struct EncodedPacket {
    data: Bytes,           // Cheap clone = ref count bump
    stream_type: StreamType,
    pts: i64,
    dts: Option<i64>,
    keyframe: bool,
}

// When capturing, clone is O(1), not a 14MB copy
let packet_clone = packet.clone(); // Just bumps ref count
```

**Key locations:**
- `crates/liteclip-core/src/encode/encoder_mod/types.rs` - EncodedPacket definition
- `crates/liteclip-core/src/buffer/ring/spmc_ring.rs` - Buffer stores Bytes
- `crates/liteclip-core/src/output/mp4.rs` - Muxing uses Bytes

### Replay Buffer Eviction

The ring buffer uses proactive eviction at 80% memory watermark to prevent mutex storms:

```rust
// Constants in spmc_ring.rs
const PROACTIVE_EVICTION_WATERMARK: f32 = 0.80;  // Start evicting early
const EVICTION_BATCH_SIZE: usize = 8;            // Batch to reduce contention

// Eviction triggers
- Duration-based (primary): Evict packets older than configured duration
- Memory-based (safety): Evict when approaching max_memory_bytes
- Proactive: At 80% memory, smooth eviction across pushes
```

**Memory limits:**
- `max_memory_bytes`: Configurable via `general.replay_memory_limit_mb`
- `MAX_OUTSTANDING_SNAPSHOT_BYTES`: 512MB max for in-flight snapshots

## Common Development Tasks

### Adding a New Encoder Type

1. Add enum variant to `EncoderType` in `crates/liteclip-core/src/config/config_mod/types.rs`
2. Create encoder module in `crates/liteclip-core/src/encode/ffmpeg/`
3. Add encoder detection in `crates/liteclip-core/src/encode/encoder_mod/functions.rs::detect_available_encoder()`
4. Add encoder options in `crates/liteclip-core/src/encode/ffmpeg/options.rs`
5. Update `crates/liteclip-core/src/encode/ffmpeg/mod.rs` to route to new encoder
6. Add UI option in `src/gui/settings.rs`

### Adding a New Configuration Option

1. Add field to appropriate config struct in `crates/liteclip-core/src/config/config_mod/types.rs`
2. Add default function in `crates/liteclip-core/src/config/config_mod/functions.rs`
3. Add `#[serde(default = "default_xxx")]` attribute
4. Add validation in `Config::validate()` if needed
5. Add UI control in `src/gui/settings.rs`

### Adding a New GUI Panel

1. Create module in `src/gui/`
2. Implement `egui::Widget` or use `egui::CentralPanel`
3. Register in `src/gui/manager.rs::GuiManager`
4. Add navigation/menu entry

### Debugging Capture Issues

1. **Enable verbose logging:**
   ```powershell
   $env:RUST_LOG = "debug,liteclip_core=trace"
   cargo run
   ```

2. **DXGI access lost:** Check for secure desktop (UAC prompts, lock screen). The capture thread handles `DXGI_ERROR_ACCESS_LOST` by releasing resources and attempting reacquisition.

3. **Frame drops:** Check backpressure state. If the encode thread can't keep up, `BackpressureState` signals the capture thread to drop frames.

4. **GPU conversion unavailable:** Check NV12 texture pool. If GPU NV12 conversion fails, it falls back to CPU readback with 2-second retry backoff.

## Gotchas & Pitfalls

### FFmpeg DLL Requirements

- FFmpeg 6.0+ shared DLLs must be placed **next to the executable** or on DLL search path
- Required DLLs: `avcodec-*.dll`, `avformat-*.dll`, `avutil-*.dll`, `swscale-*.dll`, `swresample-*.dll`, `avfilter-*.dll`
- The `build.rs` script copies DLLs from `ffmpeg_dev/` during build
- Major version must match the version linked against in `ffmpeg-next`

### DXGI Access Lost

When DXGI loses access (secure desktop, UAC, lock screen), the capture thread receives `DXGI_ERROR_ACCESS_LOST`. The code in `capture/dxgi/capture.rs` handles this by:
1. Releasing the desktop duplication
2. Waiting for reacquisition
3. Retrying the capture loop

**Don't** panic on access lost - it's expected when the desktop switches.

### Hardware Encoder Fallback

If a hardware encoder fails, the code falls back to software encoding:

```rust
// In output/video_file.rs
fn should_fallback_to_software_encoder(err: &anyhow::Error) -> bool {
    // Checks for hardware-specific errors
}
```

When testing hardware encoding, check logs for unexpected CPU fallback messages.

### Memory Pressure with Multiple Snapshots

The buffer tracks outstanding snapshot bytes. If too many concurrent save operations are in-flight, `snapshot_from()` returns an error to prevent unbounded memory growth.

```rust
// Prevents OOM when saving multiple clips simultaneously
const MAX_OUTSTANDING_SNAPSHOT_BYTES: usize = 512 * 1024 * 1024;
```

## Configuration

**Location:** `%APPDATA%\liteclip-replay\liteclip-replay.toml`

**Loading flow:**
1. `Config::load()` reads from `AppDirs::liteclip_replay()`
2. If missing, creates defaults and writes the file
3. `Config::validate()` normalizes and validates values

**Key config types:**
```rust
Config
├── general: GeneralConfig      // replay_duration_secs, save_directory, etc.
├── video: VideoConfig          // encoder, codec, framerate, bitrate
├── audio: AudioConfig          // capture_system, capture_mic, volumes
├── hotkeys: HotkeyConfig       // save_clip, toggle_recording, gallery
└── advanced: AdvancedConfig    // developer settings
```

## Health Monitoring

The main event loop polls `enforce_pipeline_health()` to detect and recover from pipeline failures:

```rust
// In main.rs event loop
match app_state_blocking_try(&app_state, |s| s.enforce_pipeline_health()) {
    Ok(Some(message)) => {
        // Pipeline recovered, show notification
    }
    Err(e) => {
        // Fatal error, may need restart
    }
    Ok(None) => {
        // Healthy, nothing to do
    }
}
```

**Location:** `crates/liteclip-core/src/app/state.rs::enforce_pipeline_health()`

## Testing

### Running Tests

```bash
# All tests
cargo test

# Specific test
cargo test test_snapshot_cheap_clone

# With output
cargo test -- --nocapture

# Fast compile check
cargo test --no-run
```

### Test Locations

- Unit tests: Inline in `#[cfg(test)]` modules within source files
- Integration tests: `crates/liteclip-core/tests/`
- Examples: `crates/liteclip-core/examples/` (runnable demos)

### Testing Hardware Encoding Without GPU

The software encoder (`sw_encoder.rs`) works on any system. To test hardware encoder logic without a GPU:
1. Set encoder to `Auto` - will fall back to software
2. Check logs for fallback messages
3. Test encoder selection with `detect_available_encoder()`

## Dependencies

| Dependency | Version | Purpose |
|------------|---------|---------|
| Rust | 1.70+ | Language toolchain |
| FFmpeg | 6.0+ | Video encoding/muxing |
| Windows SDK | 10+ | Windows API bindings |
| Visual Studio Build Tools | 2022 | C++ toolchain for native deps |

## Release Process

Automated via `.github/workflows/release.yml`:

1. Tag with `v*` pattern (e.g., `v0.2.0`)
2. Workflow builds MSI via WiX
3. GitHub release created with MSI + portable ZIP artifacts
4. Release notes auto-generated from commits

## Debugging & Profiling

### Enable Verbose Logging

```powershell
# PowerShell
$env:RUST_LOG = "debug,liteclip_core=trace,wgpu=warn,naga=warn"
cargo run
```

### Memory Profiling on Windows

The codebase includes memory diagnostics in `crates/liteclip-core/src/memory_diag.rs`:
- Use Windows Performance Recorder for heap analysis
- Visual Studio Diagnostic Tools for memory snapshots
- Intel VTune for memory growth attribution

### GPU Debugging

- Check D3D11 debug layer: Enable via DirectX Control Panel
- NVENC errors: Check NVIDIA driver logs
- D3D11VA decode: Gallery uses generic D3D11VA (not vendor-specific decode)

## Embedding liteclip-core

The `liteclip-core` crate is designed for embedding in other applications:

```rust
use liteclip_core::{ReplayEngine, ReplayEngineBuilder, paths::AppDirs};

// Create engine with custom app slug
let dirs = AppDirs::from_app_slug("my-app")?;
let engine = ReplayEngine::builder(dirs)
    .build()?;

// Start recording
engine.state_mut().start_recording().await?;

// Poll health in your UI loop
if let Some(message) = engine.enforce_pipeline_health()? {
    // Show recovery notification
}

// Save clip
let path = engine.save_clip(duration, None, None).await?;
```

**Examples:** `crates/liteclip-core/examples/`
- `minimal_engine.rs` - Basic start/stop/save
- `engine_host.rs` - With CoreHost callbacks
- `custom_paths.rs` - Custom config paths
