# LiteClip Replay - Architecture Refactor Plan

## Executive Summary

**Scope:** Refactor only - maintain current functionality, focus on code quality and architecture.
**GUI:** Keep egui + wgpu.
**Testing:** Minimal - focus on architecture improvements.

**Current State:** Functional prototype with working capture, encoding, and replay buffer.
**Target State:** Production-ready, maintainable, well-organized codebase.

---

## 1. Priority Refactoring Areas

### 1.1 Critical (Do First)

| Area | Current State | Target State | Lines Changed |
|------|---------------|--------------|---------------|
| `app.rs` | 554-line god file | Split into `app/state.rs`, `app/pipeline/` | ~600 |
| `encode/ffmpeg_encoder.rs` | 1108-line monolith | Split by encoder type | ~1200 |
| Error handling | Generic `anyhow::Error` | Domain-specific error types | ~300 |
| Module naming | `encoder_mod`, `config_mod` | Clean `encode/`, `core/` | ~50 |

### 1.2 Important (Do Second)

| Area | Issue | Fix |
|------|-------|-----|
| `SharedReplayBuffer` | Unnecessary wrapper | Rename `LockFreeReplayBuffer` directly |
| Config trait files | 6 separate `*_traits.rs` files | Consolidate into `core/config.rs` |
| `capture/dxgi/functions.rs` | ~500 lines | Split into `capture.rs`, `device.rs`, `texture.rs` |

### 1.3 Optional Improvements

- Add structured metrics (frames captured, latency, etc.)
- Add lifecycle state machine for cleaner state transitions
- Add crash reporting

---

## 2. Proposed Directory Structure

```
src/
├── lib.rs
├── main.rs
│
├── core/                    # NEW: Consolidated domain types
│   ├── mod.rs
│   ├── error.rs             # Domain error hierarchy
│   ├── config.rs            # All config types (consolidated)
│   └── types.rs             # EncodedPacket, StreamType, etc.
│
├── capture/
│   ├── mod.rs
│   ├── error.rs             # NEW: Capture-specific errors
│   ├── frame.rs             # CapturedFrame, D3d11Frame
│   ├── dxgi/
│   │   ├── mod.rs
│   │   ├── capture.rs       # Split from functions.rs
│   │   ├── device.rs        # D3D11 device management
│   │   └── texture.rs       # Texture pool
│   └── audio/
│       ├── mod.rs
│       ├── manager.rs
│       ├── system.rs
│       ├── mic.rs
│       └── mixer.rs         # NEW: Extract from manager.rs
│
├── encode/
│   ├── mod.rs
│   ├── error.rs             # NEW: Encoder-specific errors
│   ├── packet.rs            # Moved from encoder_mod/types.rs
│   ├── config.rs            # EncoderConfig
│   ├── spawn.rs             # Thread spawning (from encoder_mod/functions.rs)
│   └── ffmpeg/              # NEW: Split ffmpeg_encoder.rs
│       ├── mod.rs
│       ├── context.rs       # D3D11 hardware context
│       ├── amf.rs           # AMF-specific logic
│       ├── nvenc.rs         # NVENC-specific logic
│       ├── qsv.rs           # QSV-specific logic
│       ├── software.rs      # CPU encoding
│       └── options.rs       # Encoder option builders
│
├── buffer/
│   ├── mod.rs
│   ├── error.rs             # NEW
│   ├── ring.rs              # Renamed from lockfree.rs
│   └── stats.rs
│
├── output/                  # Renamed from clip/
│   ├── mod.rs
│   ├── error.rs             # NEW
│   ├── mp4.rs               # Renamed from ffmpeg_muxer.rs
│   └── thumbnail.rs
│
├── platform/                # Keep mostly as-is
│   ├── mod.rs
│   ├── error.rs             # NEW
│   ├── hotkey.rs
│   ├── tray.rs
│   └── autostart.rs
│
├── gui/                     # Keep egui + wgpu
│   ├── mod.rs
│   ├── settings.rs          # Keep but could split later
│   └── manager.rs
│
└── app/                     # NEW: Split from app.rs
    ├── mod.rs
    ├── state.rs             # Minimal AppState
    ├── clip.rs              # ClipManager
    └── pipeline/
        ├── mod.rs
        ├── manager.rs       # RecordingPipeline
        ├── lifecycle.rs     # Lifecycle state machine
        ├── video.rs         # Video capture + encode
        └── audio.rs         # Audio capture
```

---

## 3. Current Architecture Issues

### 3.1 Code Organization Issues

| Issue | Location | Severity | Description |
|-------|----------|----------|-------------|
| Inconsistent module structure | `encode/encoder_mod/`, `config/config_mod/` | High | Mix of flat files and `mod.rs` patterns with confusing `functions.rs`, `types.rs`, `*_traits.rs` splits |
| Poor naming | `encoder_mod` | Medium | Module name `encoder_mod` is unclear; suggests "modifier" rather than "module" |
| Redundant re-exports | `encode/mod.rs`, `clip/muxer/mod.rs` | Low | Multiple `pub use` statements create confusion about actual source location |
| Deep nesting | `config/config_mod/videoconfig_traits.rs` | Medium | 4-level deep file paths for simple impl blocks |
| God struct | `app.rs:AppState` | High | Single struct manages config, buffer, pipeline, and orchestrates everything |

### 3.2 API Design Issues

| Issue | Location | Severity | Description |
|-------|----------|----------|-------------|
| Wrapper without value | `buffer/ring/types.rs:SharedReplayBuffer` | Medium | Wraps `LockFreeReplayBuffer` with no actual thread-safety logic; just delegates |
| Incomplete trait | `capture/mod.rs:CaptureBackend` | Medium | Trait only used once; not leveraged for abstraction |
| Manual lifecycle | `app.rs:RecordingLifecycle` | Medium | State machine manually managed with 5 states; error-prone |
| Mixed concerns | `app.rs:RecordingPipeline` | High | Manages capture, encoder, audio, and their health monitoring |

### 3.3 Error Handling Issues

| Issue | Location | Severity | Description |
|-------|----------|----------|-------------|
| Generic errors | Throughout | High | Uses `anyhow::Error` everywhere; no domain-specific error types |
| Lost context | `app.rs:rollback_startup()` | Medium | Error context lost during cleanup |
| Silent failures | `main.rs:shutdown watchdog` | Medium | Force-exit on timeout loses error information |

---

## 4. Error Handling Design

```rust
// src/core/error.rs
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LiteClipError {
    #[error("Configuration error: {0}")]
    Config(#[from] ConfigError),
    
    #[error("Capture error: {0}")]
    Capture(#[from] CaptureError),
    
    #[error("Encoding error: {0}")]
    Encode(#[from] EncodeError),
    
    #[error("Buffer error: {0}")]
    Buffer(#[from] BufferError),
    
    #[error("Output error: {0}")]
    Output(#[from] OutputError),
}

// src/capture/error.rs
#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("DXGI error: {0}")]
    Dxgi(#[from] DxgiError),
    
    #[error("Audio error: {0}")]
    Audio(#[from] AudioError),
    
    #[error("Capture not initialized")]
    NotInitialized,
}

#[derive(Debug, Error)]
pub enum DxgiError {
    #[error("Access denied - ensure not running as service")]
    AccessDenied,
    
    #[error("GPU device lost: {reason}")]
    DeviceLost { reason: String },
    
    #[error("Desktop unavailable (output {output_index})")]
    DesktopUnavailable { output_index: u32 },
}

// src/encode/error.rs
#[derive(Debug, Error)]
pub enum EncodeError {
    #[error("Codec not found: {0}")]
    CodecNotFound(String),
    
    #[error("Encoder initialization failed: {0}")]
    InitializationFailed(String),
    
    #[error("Encoding failed: {0}")]
    EncodingFailed(String),
    
    #[error("Hardware encoder error: {0}")]
    HardwareError(String),
}

// src/buffer/error.rs
#[derive(Debug, Error)]
pub enum BufferError {
    #[error("Buffer capacity exceeded")]
    CapacityExceeded,
    
    #[error("No keyframe available")]
    NoKeyframe,
    
    #[error("Buffer is empty")]
    Empty,
}

// src/output/error.rs
#[derive(Debug, Error)]
pub enum OutputError {
    #[error("Muxer error: {0}")]
    MuxerError(String),
    
    #[error("File write error: {0}")]
    FileWriteError(#[from] std::io::Error),
    
    #[error("No video packets")]
    NoVideoPackets,
}
```

---

## 5. Implementation Phases

### Phase 1: Foundation (3 days)

**Goal:** Create core module and error hierarchy

**Tasks:**
- [ ] Create `src/core/mod.rs`
- [ ] Create `src/core/error.rs` with domain error types
- [ ] Move config types to `src/core/config.rs`
- [ ] Move `EncodedPacket`, `StreamType` to `src/core/types.rs`
- [ ] Delete `config/config_mod/*_traits.rs` files
- [ ] Update all imports

**Files Changed:**
- `src/lib.rs` - Add `mod core;`
- `src/core/mod.rs` - New file
- `src/core/error.rs` - New file
- `src/core/config.rs` - Consolidated from `config/config_mod/`
- `src/core/types.rs` - New file
- `src/config/mod.rs` - Re-export from core

### Phase 2: Encode Restructure (5 days)

**Goal:** Split `ffmpeg_encoder.rs` by encoder type

**Tasks:**
- [ ] Create `src/encode/ffmpeg/` subdirectory
- [ ] Extract D3D11 hardware context to `context.rs`
- [ ] Extract AMF logic to `amf.rs`
- [ ] Extract NVENC logic to `nvenc.rs`
- [ ] Extract QSV logic to `qsv.rs`
- [ ] Extract software encoding to `software.rs`
- [ ] Extract encoder option builders to `options.rs`
- [ ] Move `encoder_mod/types.rs` to `encode/packet.rs`
- [ ] Move `encoder_mod/functions.rs` to `encode/spawn.rs`
- [ ] Delete `encoder_mod/` directory
- [ ] Update all imports

**File Splits:**

```
ffmpeg_encoder.rs (1108 lines)
├── context.rs       (~150 lines) - D3d11HardwareContext, AvD3d11vaDeviceContext
├── amf.rs           (~200 lines) - AMF-specific init and encoding
├── nvenc.rs         (~200 lines) - NVENC-specific init and encoding
├── qsv.rs           (~150 lines) - QSV-specific init and encoding
├── software.rs      (~100 lines) - CPU encoding path
├── options.rs       (~150 lines) - Encoder option builders
└── mod.rs           (~150 lines) - FfmpegEncoder struct, common code
```

### Phase 3: Capture Restructure (3 days)

**Goal:** Split large DXGI file and extract audio mixer

**Tasks:**
- [ ] Split `capture/dxgi/functions.rs`:
  - [ ] Extract capture loop to `capture.rs`
  - [ ] Extract device management to `device.rs`
  - [ ] Extract texture pool to `texture.rs`
- [ ] Extract audio mixer from `manager.rs` to `mixer.rs`
- [ ] Create `capture/error.rs`
- [ ] Move `CapturedFrame`, `D3d11Frame` to `capture/frame.rs`

**File Splits:**

```
functions.rs (~500 lines)
├── capture.rs   (~200 lines) - Main capture loop
├── device.rs    (~150 lines) - D3D11 device creation
└── texture.rs   (~150 lines) - Texture pool and NV12 conversion
```

### Phase 4: App Split (4 days)

**Goal:** Split `app.rs` into focused modules

**Tasks:**
- [ ] Create `src/app/` directory
- [ ] Create `app/mod.rs` with re-exports
- [ ] Move `AppState` to `app/state.rs` (minimal)
- [ ] Move `RecordingPipeline` to `app/pipeline/manager.rs`
- [ ] Move `ClipManager` to `app/clip.rs`
- [ ] Create `app/pipeline/lifecycle.rs` with state machine
- [ ] Create `app/pipeline/video.rs` for video capture/encode
- [ ] Create `app/pipeline/audio.rs` for audio capture
- [ ] Update all imports

**File Splits:**

```
app.rs (554 lines)
├── state.rs              (~80 lines) - Minimal AppState
├── clip.rs               (~70 lines) - ClipManager
└── pipeline/
    ├── mod.rs            (~20 lines) - Re-exports
    ├── manager.rs        (~200 lines) - RecordingPipeline
    ├── lifecycle.rs      (~50 lines) - Lifecycle state machine
    ├── video.rs          (~80 lines) - Video pipeline helpers
    └── audio.rs          (~50 lines) - Audio pipeline helpers
```

### Phase 5: Buffer & Output Cleanup (2 days)

**Goal:** Simplify buffer API and rename output module

**Tasks:**
- [ ] Rename `LockFreeReplayBuffer` to `ReplayBuffer`
- [ ] Remove `SharedReplayBuffer` wrapper
- [ ] Update `buffer/mod.rs` exports
- [ ] Rename `clip/` to `output/`
- [ ] Rename `ffmpeg_muxer.rs` to `mp4.rs`
- [ ] Create `output/error.rs`

### Phase 6: Final Cleanup (2 days)

**Goal:** Clean up imports and documentation

**Tasks:**
- [ ] Run `cargo clippy --all-targets -- -D warnings`
- [ ] Run `cargo fmt -- --check`
- [ ] Update all module documentation
- [ ] Remove deprecated re-exports
- [ ] Update `lib.rs` with new module structure
- [ ] Verify build succeeds
- [ ] Test basic functionality

---

## 6. Success Metrics

| Metric | Current | Target |
|--------|---------|--------|
| Largest file | 1108 lines (`ffmpeg_encoder.rs`) | <300 lines |
| `app.rs` size | 554 lines | <100 lines |
| Error types | 1 (`anyhow::Error`) | 6+ domain types |
| Module depth | 4 levels (`config/config_mod/`) | 2 levels max |
| Files > 500 lines | 4 files | 0 files |

---

## 7. File Size Targets

| Current File | Lines | Target | Action |
|--------------|-------|--------|--------|
| `encode/ffmpeg_encoder.rs` | 1108 | <300 | Split into `ffmpeg/` subdirectory |
| `app.rs` | 554 | <100 | Split into `app/pipeline/` |
| `capture/dxgi/functions.rs` | ~500 | <200 | Split into capture/, device/, texture/ |
| `gui/settings.rs` | 335 | <200 | Consider split by settings category |
| `clip/muxer/ffmpeg_muxer.rs` | 514 | <200 | Split muxer/, audio/, video/ |
| `buffer/ring/lockfree.rs` | 524 | <400 | Extract parameter caching |
| `capture/audio/manager.rs` | 282 | <200 | Extract mixer logic |

---

## 8. Risks & Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Breaking changes during refactor | Medium | High | Keep compatibility re-exports during transition |
| Performance regression | Low | High | Test encoding throughput before/after |
| GUI breakage | Low | Medium | Keep egui code isolated, minimal changes |
| Import hell | Medium | Low | Use IDE refactoring tools, update incrementally |

---

## 9. Dependency Audit

| Dependency | Version | Purpose | Action |
|------------|---------|---------|--------|
| `ffmpeg-next` | 7.1.0 | Video encoding | Keep (core) |
| `egui` | 0.33.3 | Settings GUI | Keep |
| `eframe` | 0.33.3 | GUI framework | Keep |
| `tokio` | 1.x | Async runtime | Keep |
| `crossbeam` | 0.8 | Channels | Keep |
| `anyhow` | 1.x | Top-level errors | Keep |
| `thiserror` | 1.x | Error derives | Keep |
| `tracing` | 0.1 | Logging | Keep |
| `serde` | 1.x | Serialization | Keep |
| `bytes` | 1.x | Zero-copy buffers | Keep |
| `parking_lot` | 0.12 | Mutexes | Keep |

---

## 10. Testing Strategy

Since testing is minimal priority:

1. **Manual testing after each phase:**
   - Start recording
   - Save clip
   - Verify MP4 plays
   - Check audio sync

2. **Build verification:**
   - `cargo build --release`
   - `cargo clippy -- -D warnings`

3. **No new tests required** - existing tests should continue to pass

---

## 11. Rollback Plan

If issues arise:

1. Keep git tags for each phase completion
2. Can revert to previous stable state
3. Each phase is independently mergeable

---

## 12. Next Steps

1. **Phase 1** is ready to begin - creates foundation without breaking changes
2. Start with `src/core/error.rs` to define error hierarchy
3. Then consolidate config types into `src/core/config.rs`

---

## Appendix: Detailed File Changes

### Phase 1 File Changes

```
NEW FILES:
src/core/mod.rs
src/core/error.rs
src/core/config.rs
src/core/types.rs

MODIFIED FILES:
src/lib.rs
src/config/mod.rs

DELETED FILES:
src/config/config_mod/videoconfig_traits.rs
src/config/config_mod/audioconfig_traits.rs
src/config/config_mod/hotkeyconfig_traits.rs
src/config/config_mod/generalconfig_traits.rs
src/config/config_mod/advancedconfig_traits.rs
```

### Phase 2 File Changes

```
NEW FILES:
src/encode/ffmpeg/mod.rs
src/encode/ffmpeg/context.rs
src/encode/ffmpeg/amf.rs
src/encode/ffmpeg/nvenc.rs
src/encode/ffmpeg/qsv.rs
src/encode/ffmpeg/software.rs
src/encode/ffmpeg/options.rs
src/encode/error.rs
src/encode/packet.rs
src/encode/config.rs
src/encode/spawn.rs

MODIFIED FILES:
src/encode/mod.rs

DELETED FILES:
src/encode/ffmpeg_encoder.rs (split into ffmpeg/)
src/encode/encoder_mod/ (entire directory)
```

### Phase 3 File Changes

```
NEW FILES:
src/capture/error.rs
src/capture/frame.rs
src/capture/dxgi/capture.rs
src/capture/dxgi/device.rs
src/capture/dxgi/texture.rs
src/capture/audio/mixer.rs

MODIFIED FILES:
src/capture/mod.rs
src/capture/audio/manager.rs

DELETED FILES:
src/capture/dxgi/functions.rs (split)
```

### Phase 4 File Changes

```
NEW FILES:
src/app/mod.rs
src/app/state.rs
src/app/clip.rs
src/app/pipeline/mod.rs
src/app/pipeline/manager.rs
src/app/pipeline/lifecycle.rs
src/app/pipeline/video.rs
src/app/pipeline/audio.rs

DELETED FILES:
src/app.rs (split into app/)
```

### Phase 5 File Changes

```
RENAMED:
src/buffer/ring/lockfree.rs → src/buffer/ring.rs
src/clip/ → src/output/
src/clip/muxer/ffmpeg_muxer.rs → src/output/mp4.rs

MODIFIED:
src/buffer/mod.rs
src/buffer/ring/types.rs (deleted, merged into ring.rs)

DELETED:
src/buffer/ring/types.rs (SharedReplayBuffer wrapper)
```