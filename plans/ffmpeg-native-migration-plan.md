# FFmpeg CLI -> Native FFmpeg Migration Plan

## Objective
Complete the migration from spawning `ffmpeg` as a subprocess (CLI mode) to using native FFmpeg APIs through `ffmpeg-next` for recording + clip muxing, while preserving feature parity (hardware encoding behavior, A/V sync, and output quality).

## Current Snapshot (What Is Already Done)

### 1) Native FFmpeg is linked and initialized
- `ffmpeg-next` is present in dependencies.
  - Evidence: @Cargo.toml#54-54
- App startup initializes FFmpeg natively via `ffmpeg_next::init()`.
  - Evidence: @src/main.rs#61-63

### 2) Encoder creation path is currently native
- `create_encoder()` returns `FfmpegEncoder` (native API path) instead of a CLI-backed encoder.
  - Evidence: @src/encode/encoder_mod/functions.rs#95-98
- The recording pipeline uses `spawn_encoder_with_receiver(...)` in capture mode (DXGI -> encoder), not the old CLI pull mode.
  - Evidence: @src/app.rs#225-227

### 3) Legacy hardware pull mode is disabled at runtime
- `should_use_hardware_pull_mode()` hard-returns `false` with comments marking old pull mode obsolete.
  - Evidence: @src/app.rs#61-65

### 4) Clip finalization is already native muxer-based
- `Muxer::finalize_ffmpeg()` constructs `FfmpegMuxer` and writes packets natively.
  - Evidence: @src/clip/muxer/types.rs#150-160
- Native muxer implementation exists in `ffmpeg_muxer.rs`.
  - Evidence: @src/clip/muxer/ffmpeg_muxer.rs#8-17

## Remaining Work (What Is Left)

## P0 - Remove active CLI coupling from codebase

### A) Replace/remove CLI-based hardware capability probing
- `detect_hardware_encoder()` currently calls `hw_encoder::check_encoder_available(...)`.
  - Evidence: @src/encode/encoder_mod/functions.rs#50-74
- `check_encoder_available(...)` shells out to FFmpeg CLI (`Command::new(...).arg("-encoders") ...`).
  - Evidence: @src/encode/hw_encoder/functions.rs#199-207

**Plan**
1. Implement native encoder capability probing (via `ffmpeg_next` codec discovery/open test).
2. Switch `detect_hardware_encoder()` to native probe results.
3. Remove CLI probe function usage.

### B) Decommission legacy CLI encoder module from production path
- `hw_encoder` still contains a full subprocess pipeline (`Command`, `Stdio`, process management, stdout/stderr readers).
  - Evidence: @src/encode/hw_encoder/types.rs#91-100
  - Evidence: @src/encode/hw_encoder/types.rs#151-161
  - Evidence: @src/encode/hw_encoder/types.rs#258-261

**Plan**
1. Mark `encode/hw_encoder/*` as deprecated/internal migration leftovers.
2. Remove dead exports from `encode/mod.rs` once no callsites remain.
3. Delete module after parity + tests are complete.

## P0 - Finish native muxer parity (audio + container behavior)

### C) Implement audio stream writing in native muxer
- `FfmpegMuxer` has `_audio_stream_index` and `write_packets(..., _audio_packets)` where audio is currently ignored.
  - Evidence: @src/clip/muxer/ffmpeg_muxer.rs#11-12
  - Evidence: @src/clip/muxer/ffmpeg_muxer.rs#57-57
- `Muxer` already buffers audio packets, but native muxer path does not write them yet.
  - Evidence: @src/clip/muxer/types.rs#47-50
  - Evidence: @src/clip/muxer/types.rs#158-158

**Plan**
1. Add audio stream creation (AAC or chosen codec) with explicit time base.
2. Rescale audio packet PTS/DTS and interleave audio/video packets.
3. Respect `expect_audio` behavior (silence/fallback semantics).
4. Add regression tests for system-only, mic-only, and mixed audio clips.

### D) Wire `faststart` and muxer config options fully
- `MuxerConfig.faststart` exists but is not enforced in `FfmpegMuxer` output options today.
  - Evidence: @src/clip/muxer/types.rs#181-183

**Plan**
1. Apply `movflags=+faststart` (or equivalent native API dictionary option).
2. Verify playback/startup behavior in browser/media players.

## P1 - Simplify encoder config semantics after CLI removal

### E) Revisit `use_cpu_readback` + `output_index` semantics
- These fields still reflect old CLI pull/capture split and desktop-grab index assumptions.
  - Evidence: @src/encode/encoder_mod/types.rs#106-109
  - Evidence: @src/encode/encoder_mod/encoderconfig_traits.rs#33-35

**Plan**
1. Rename/re-scope settings to match native architecture.
2. Remove config paths that only existed for CLI pull mode.
3. Update settings UI labels/help text accordingly.

### F) Clean up outdated init API surface
- `encode::init_ffmpeg()` currently logs "initialization skipped" and is misleading now that native init occurs in `main.rs`.
  - Evidence: @src/encode/encoder_mod/functions.rs#372-375

**Plan**
1. Either remove this helper or make it call `ffmpeg_next::init()`.
2. Ensure there is a single source of truth for FFmpeg init.

## P1 - Packaging/build migration cleanup

### G) Remove bundled CLI executable dependency
- Installer explicitly ships `liteclip-replay-ffmpeg.exe`.
  - Evidence: @installer/Components.wxs#20-22

**Plan**
1. Decide runtime strategy for native FFmpeg libs (static vs bundled DLLs).
2. Remove CLI exe component from installer when no codepath requires it.
3. Keep only required FFmpeg runtime libraries/artifacts.

## P2 - Docs and dead-code cleanup

### H) Update architecture docs/comments that still describe subprocess pipeline
- `CLAUDE.md` still says encoding is via FFmpeg subprocess and describes obsolete modes.
  - Evidence: @CLAUDE.md#45-49

**Plan**
1. Update architecture docs to native encoder/muxer pipeline.
2. Remove stale comments and "Phase 1/Phase 2" notes no longer accurate.

## Recommended Execution Order
1. **Muxer parity first (audio + faststart)**
2. **Native hardware probing + remove CLI probe usage**
3. **Delete/deprecate legacy `hw_encoder` subprocess implementation**
4. **Config/UI cleanup (`use_cpu_readback`, `output_index`)**
5. **Installer + docs cleanup**

## Definition of Done
- No runtime `Command::new` calls remain for FFmpeg encoding/muxing/probing paths.
- Native muxer writes both video and audio tracks correctly.
- Hardware encoder detection is native-only.
- Installer no longer ships FFmpeg CLI executable unless explicitly needed for a separate feature.
- Documentation accurately reflects native FFmpeg architecture.
- Regression matrix passes:
  - H.264/H.265/AV1 where supported
  - NVENC/AMF/QSV/Software selection
  - system audio only / mic only / both
  - long recording A/V sync validation
