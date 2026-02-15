# AGENTS.md

This file provides guidance to agents when working with code in this repository.

## Project Context

LiteClip Recorder is a Windows-only screen capture application using D3D11/DXGI Desktop Duplication. It captures desktop frames, encodes them using hardware encoders (NVENC/AMF/QSV) via FFmpeg CLI or software JPEG encoding, stores them in a memory-bounded ring buffer, and saves clips on hotkey trigger.

## Build Commands

Standard Cargo commands work. The `ffmpeg` feature flag enables FFmpeg code paths but requires FFmpeg at runtime (not link-time):

```bash
cargo build --release --features ffmpeg
cargo run --features ffmpeg
```

**Critical:** Release profile uses `lto = "fat"` and `panic = "abort"` - expect longer compile times for optimized binaries.

## Platform Requirements

- Windows 10+ with DXGI 1.2 support
- Windows SDK (for D3D11 headers)
- FFmpeg in PATH or at expected locations (checked in order: `LITECLIP_FFMPEG_PATH` env var, `./ffmpeg/bin/ffmpeg.exe`, `<exe_dir>/ffmpeg/bin/ffmpeg.exe`, system PATH)

## Critical Code Patterns

### Hardware Encoder Selection
Encoder selection happens in [`encode/mod.rs`](src/encode/mod.rs) but actual FFmpeg command building with encoder-specific flags is in [`encode/hw_encoder.rs`](src/encode/hw_encoder.rs). Each encoder requires different flags:

- **h264_nvenc**: Uses `preset=p4`, `tune=ll` (low latency), `rc=vbr`, `cq=23`
- **h264_amf**: **CRITICAL** - requires `-bf 0` (disable B-frames), `-sei +aud`, `-vsync cfr`. Missing B-frame disable produces unplayable output.
- **h264_qsv**: Uses `preset=veryfast`

Encoder initialization is lazy - happens on first frame, not at encoder creation.

### Frame Data Flow
[`CapturedFrame`](src/capture/mod.rs) contains **both** a D3D11 texture handle AND CPU BGRA bytes. Even when using hardware encoding that only needs the GPU texture, CPU readback happens unconditionally (Phase 1 limitation). This means unnecessary memory copies.

### Ring Buffer Eviction
The [`ReplayBuffer`](src/buffer/ring.rs) evicts based on **memory bytes**, not duration. The `duration` field is informational only. High-bitrate content means fewer seconds stored. Keyframe indices are rebuilt O(N) on every eviction which can stall under memory pressure.

### Error Handling
Many error paths log with `warn!`/`error!` and continue silently rather than propagating. The encoder thread may be dead but the application continues running. Always check thread join results.

### Configuration
Config stored at `%APPDATA%/liteclip-replay/liteclip-replay.toml`. Resolution config is ignored if `use_native_resolution` is true - actual resolution comes from the first captured frame.

## Testing

No special test configuration. Standard `cargo test` works, though hardware encoder tests require FFmpeg and compatible GPU.

## Dependencies

- `windows` crate with many Win32 features enabled - see Cargo.toml for full list
- `tokio` for async runtime
- `crossbeam` for thread channels
- `parking_lot` for synchronization primitives
- `bytes` for ref-counted buffers (used throughout for cheap cloning)
