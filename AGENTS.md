# AGENTS.md

This file provides guidance to agents when working with code in this repository.

## Project Overview
LiteClip Replay - A Windows-only screen recording application with replay buffer. Captures via DXGI Desktop Duplication, encodes with FFmpeg (optional), and saves clips on hotkey trigger.

## Build Commands
```powershell
# Build without FFmpeg (MP4 muxing stubbed)
cargo build

# Build with FFmpeg support (required for playable MP4 output)
cargo build --features ffmpeg

# Run with FFmpeg
cargo run --features ffmpeg

# Run tests
cargo test

# Run a specific test
cargo test test_name
```

## Critical Architecture Patterns

### Threading Model
- **Main thread**: Async tokio runtime, handles AppState and clip saving
- **Platform thread**: Hidden Win32 window (`HWND_MESSAGE`) running `GetMessage` pump for global hotkeys
- **Capture thread**: DXGI Desktop Duplication frame acquisition
- **Encoder thread**: Receives frames via `crossbeam::channel`, pushes encoded packets to ring buffer
- Events flow: `Win32 WM_HOTKEY` → `crossbeam::Sender` → `tokio::mpsc::channel` → async handler

### Hotkey System
- Uses Win32 `RegisterHotKey` API which requires a window handle
- Hidden window created with `HWND_MESSAGE` (message-only, no UI)
- Hotkey IDs are hardcoded constants in BOTH `hotkeys.rs` and `msg_loop.rs` - must stay in sync:
  - `HOTKEY_ID_SAVE_CLIP = 1000`
  - `HOTKEY_ID_TOGGLE_RECORDING = 1001`
  - `HOTKEY_ID_SCREENSHOT = 1002`
  - `HOTKEY_ID_OPEN_GALLERY = 1003`

### Ring Buffer Design
- Uses `Bytes` crate for reference-counted packet data
- `SharedReplayBuffer` wraps `Arc<RwLock<ReplayBuffer>>` with parking_lot (not std::sync)
- Snapshots are cheap: cloning the buffer only bumps ref counts, copies no data
- Memory budget enforcement: evicts old packets when `max_memory_bytes` exceeded

### Config Location
- Stored at `%APPDATA%/liteclip-replay/liteclip-replay.toml`
- Created automatically with defaults if missing
- Uses `dirs::data_dir()` for cross-Windows compatibility

### FFmpeg Integration
- FFmpeg support is **optional** via `ffmpeg` feature flag
- Without FFmpeg: clips cannot be muxed to MP4 (compile warning emitted in `main.rs`)
- All muxing code uses `#[cfg(feature = "ffmpeg")]` guards with stub fallbacks

### Encoder Shutdown Pattern
- Encoder thread is joined synchronously in `stop_recording()`:
  ```rust
  if let Some(handle) = self.encoder_handle.take() {
      match handle.thread.join() { ... }
  }
  ```
- This is intentional - ensures all packets are flushed before saving

### Windows API Dependencies
- Requires Windows SDK with DXGI, D3D11, MediaFoundation headers
- Links against: `d3d11`, `dxgi`, `dxguid`, `user32`, `gdi32` (see `build.rs`)

## Code Style
- Use `anyhow::Result` for error handling (not `Result<T, E>`)
- Tracing for logging: `info!`, `debug!`, `warn!`, `error!` (not println!)
- Win32 APIs are `unsafe` - wrap in `unsafe` blocks with context
- Platform code is Windows-only - no cross-platform abstractions
