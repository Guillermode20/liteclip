# LiteClip Recorder Architecture

## Overview

LiteClip Recorder is a native Windows desktop screen recorder built in Rust. It continuously records in the background using a replay buffer and lets users save clips on demand.

**Architecture Pattern**: Library + Application separation
- `liteclip-core`: Reusable engine library (capture, encode, buffer, output)
- `liteclip-replay`: GUI application (platform integration, user interface)

## Data Flow

```
Capture (DXGI/WASAPI) → Encode (FFmpeg) → Buffer (SPMC Ring) → Output (MP4)
```

## Threading Model

| Thread | Responsibility |
|--------|---------------|
| Main Thread | Tokio async runtime, event loop, config I/O, health monitoring |
| Platform Thread | Windows message loop, hotkey handling, tray icon |
| Capture Thread | DXGI Desktop Duplication, WASAPI audio capture |
| Encode Thread | Video/audio encoding to FFmpeg |

## Key Components

### Capture (crates/liteclip-core/src/capture/)
- **DxgiCapture**: Desktop Duplication API for screen capture
- **AudioCapture**: WASAPI for system audio (loopback) and microphone
- **BackpressureState**: Signals capture to drop frames when encode can't keep up

### Encode (crates/liteclip-core/src/encode/)
- **FfmpegEncoder**: Abstraction over FFmpeg encoding
- **Hardware encoders**: NVENC (NVIDIA), AMF (AMD), QSV (Intel)
- **Software fallback**: libx264/libx265 when hardware unavailable

### Buffer (crates/liteclip-core/src/buffer/)
- **LockFreeReplayBuffer**: SPMC ring buffer with proactive eviction
- Eviction triggers: duration-based (primary), memory-based (safety)
- Proactive eviction at 80% watermark to prevent mutex storms

### Output (crates/liteclip-core/src/output/)
- **FfmpegMuxer**: MP4 container muxing
- **spawn_clip_saver**: Asynchronous clip saving
- **SDK export**: Trimmed clip export for gallery

### App/Pipeline (crates/liteclip-core/src/app/)
- **AppState**: Central state coordinator
- **RecordingPipeline**: Orchestrates capture → encode → buffer
- **enforce_pipeline_health**: Detects and recovers from pipeline failures

### GUI (src/gui/)
- **GuiManager**: egui app wrapper
- **Gallery**: Clip browser and editor UI
- **DecodePipeline**: Video decode for preview playback
- **Settings**: Configuration UI

### Platform (src/platform/)
- **PlatformHandle**: Hotkeys, tray, autostart
- **msg_loop**: Windows message pump

## Memory Management

- `bytes::Bytes` for zero-copy packet handling (cheap clone = ref count bump)
- MAX_OUTSTANDING_SNAPSHOT_BYTES: 512MB max for in-flight snapshots
- Proactive eviction at 80% watermark

## Key Patterns

### DXGI Access Lost Handling
When DXGI loses access (UAC, lock screen, secure desktop):
1. Release desktop duplication
2. Wait for reacquisition
3. Retry capture loop

### Hardware Encoder Fallback
On hardware encoder failure:
1. Detect failure type
2. Fall back to software encoding
3. Log fallback for diagnostics

### Pipeline Health Monitoring
Main loop polls `enforce_pipeline_health()`:
1. Check capture thread health
2. Check encode thread health
3. If dead, restart pipeline
4. Notify user of recovery

### Frame Counting Semantics
The encoder maintains two frame counters with different semantics:
- `frame_count`: Total frames processed through the encode pipeline
- `encoder_frame_count`: Frames actually sent to the encoder (excluding duplicates/dropped)

**Important**: Keyframe decisions should use `encoder_frame_count` to ensure GOP alignment matches what the encoder actually sees. Using `frame_count` can cause keyframe timing drift.

### Mutex Poison Recovery Pattern
Use `unwrap_or_else(|e| e.into_inner())` for mutex poison recovery throughout the codebase. This prevents cascade failures when a thread panics while holding a mutex, allowing other threads to still access the lock.

```rust
let slot = self.slots[idx].lock().unwrap_or_else(|e| e.into_inner());
```

### Memory Ordering Strategy
- **Relaxed**: Used for performance-critical accounting (total_bytes, keyframe_count) where stale reads are acceptable
- **Release/Acquire**: Used for critical indices (write_idx, evict_frontier) to enforce memory boundedness invariants

This strategy balances performance with correctness - accounting values may be stale but memory limits are strictly enforced.

### Proactive Eviction Design
The replay buffer uses proactive eviction at 80% watermark (PROACTIVE_EVICTION_WATERMARK) with batch eviction (EVICTION_BATCH_SIZE=8) to prevent mutex storms at 100% memory. This spreads eviction overhead across multiple push operations rather than triggering sudden latency spikes.

## Configuration

Location: `%APPDATA%\liteclip-replay\liteclip-replay.toml`

Types: Config → GeneralConfig, VideoConfig, AudioConfig, HotkeyConfig, AdvancedConfig
