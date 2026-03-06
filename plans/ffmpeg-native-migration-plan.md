# FFmpeg CLI -> Native FFmpeg Migration Plan

## Objective
Complete the migration from spawning `ffmpeg` as a subprocess (CLI mode) to using native FFmpeg APIs through `ffmpeg-next` for recording + clip muxing, while preserving feature parity (hardware encoding behavior, A/V sync, and output quality).

## Current Snapshot (What Is Already Done)

### 1) Native FFmpeg is linked and initialized
- `ffmpeg-next` is present in dependencies.
  - Evidence: @Cargo.toml#54-54
- App startup initializes FFmpeg natively via `ffmpeg_next::init()`.
  - Evidence: @src/main.rs#62-62
- `encode::init_ffmpeg()` properly calls `ffmpeg_next::init()`.
  - Evidence: @src/encode/encoder_mod/functions.rs#433-436

### 2) Encoder is fully native
- `FfmpegEncoder` in `ffmpeg_encoder.rs` implements the `Encoder` trait using native FFmpeg APIs.
  - Evidence: @src/encode/ffmpeg_encoder.rs#10-22 (struct definition)
  - Evidence: @src/encode/ffmpeg_encoder.rs#236-440 (native encoder initialization)
- `create_encoder()` returns `FfmpegEncoder` (native API path).
  - Evidence: @src/encode/encoder_mod/functions.rs#160-169
- The recording pipeline uses `spawn_encoder_with_receiver(...)` in capture mode (DXGI -> native encoder).
  - Evidence: @src/app.rs#225-227
- Native encoder supports NVENC, AMF, QSV, and software encoding with full configuration options.
  - Evidence: @src/encode/ffmpeg_encoder.rs#267-413

### 3) Hardware encoder probing is native
- `detect_hardware_encoder()` uses native `probe_encoder_available()` function.
  - Evidence: @src/encode/encoder_mod/functions.rs#114-140
- `probe_encoder_available()` uses `ffmpeg::encoder::find_by_name()` and attempts to open the encoder natively.
  - Evidence: @src/encode/encoder_mod/functions.rs#51-109
- No CLI subprocess spawning for encoder detection.

### 4) Legacy hardware pull mode is disabled at runtime
- `should_use_hardware_pull_mode()` hard-returns `false` with comments marking old pull mode obsolete.
  - Evidence: @src/app.rs#61-65

### 5) Clip finalization uses native muxer
- `Muxer::finalize_ffmpeg()` constructs `FfmpegMuxer` and writes packets natively.
  - Evidence: @src/clip/muxer/types.rs#121-142
- Native muxer implementation in `ffmpeg_muxer.rs` handles MP4 creation.
  - Evidence: @src/clip/muxer/ffmpeg_muxer.rs#14-25

### 6) Audio stream writing is fully implemented
- `FfmpegMuxer` creates AAC audio stream when `expect_audio` is true.
  - Evidence: @src/clip/muxer/ffmpeg_muxer.rs#75-110
- Audio packets are mixed, resampled, and encoded to AAC.
  - Evidence: @src/clip/muxer/ffmpeg_muxer.rs#157-174
- PCM mixing handles system audio + microphone combination.
  - Evidence: @src/clip/muxer/ffmpeg_muxer.rs#263-305

### 7) Faststart is implemented
- `FfmpegMuxer` applies `movflags=+faststart` when configured.
  - Evidence: @src/clip/muxer/ffmpeg_muxer.rs#143-154

## Remaining Work (What Is Left)

## P0 - Remove dead CLI coupling code

### A) Delete legacy `hw_encoder` module
- The `hw_encoder` module contains CLI-based encoder implementations that are no longer used.
  - `check_encoder_available()` spawns FFmpeg CLI to probe encoders (unused).
    - Evidence: @src/encode/hw_encoder/functions.rs#199-290
  - `NvencEncoder`, `AmfEncoder`, `QsvEncoder` types spawn FFmpeg CLI subprocesses.
    - Evidence: @src/encode/hw_encoder/types.rs#91-100, #151-161, #258-261
- These are still exported from `encode/mod.rs` but not used in production paths.
  - Evidence: @src/encode/mod.rs#14

**Plan**
1. Remove `pub use hw_encoder::*;` from `encode/mod.rs`.
2. Mark `hw_encoder` module as `#[allow(dead_code)]` or delete entirely.
3. Remove `resolve_ffmpeg_command()` and related CLI helpers.
4. Delete the entire `src/encode/hw_encoder/` directory after verification.

## P1 - Simplify encoder config semantics after CLI removal

### B) Revisit `use_cpu_readback` + `output_index` semantics
- These fields still reflect old CLI pull/capture split and desktop-grab index assumptions.
  - Evidence: @src/encode/encoder_mod/types.rs#106-109
  - Evidence: @src/encode/encoder_mod/encoderconfig_traits.rs#33-35

**Plan**
1. Rename/re-scope settings to match native architecture.
2. Remove config paths that only existed for CLI pull mode.
3. Update settings UI labels/help text accordingly.

## P1 - Packaging/build migration cleanup

### C) Remove bundled CLI executable dependency
- Installer explicitly ships `liteclip-replay-ffmpeg.exe`.
  - Evidence: @installer/Components.wxs#20-22

**Plan**
1. Decide runtime strategy for native FFmpeg libs (static vs bundled DLLs).
2. Remove CLI exe component from installer when no codepath requires it.
3. Keep only required FFmpeg runtime libraries/artifacts.

## P2 - Docs and dead-code cleanup

### D) Update architecture docs/comments that still describe subprocess pipeline
- `CLAUDE.md` still says encoding is via FFmpeg subprocess and describes obsolete modes.
  - Evidence: @CLAUDE.md#45-49

**Plan**
1. Update architecture docs to native encoder/muxer pipeline.
2. Remove stale comments and "Phase 1/Phase 2" notes no longer accurate.
3. Update the architecture diagram to show DXGI -> Native Encoder path.

## Recommended Execution Order
1. **Delete/deprecate legacy `hw_encoder` subprocess implementation**
2. **Config/UI cleanup (`use_cpu_readback`, `output_index`)**
3. **Installer + docs cleanup**

## Definition of Done
- [x] No runtime `Command::new` calls remain for FFmpeg **encoding** paths.
- [x] Native muxer writes both video and audio tracks correctly.
- [x] Hardware encoder detection is native-only.
- [x] `faststart` option is implemented.
- [ ] `hw_encoder` module removed or marked as dead code.
- [ ] Installer no longer ships FFmpeg CLI executable unless explicitly needed for a separate feature.
- [ ] Documentation accurately reflects native FFmpeg architecture.
- [ ] Regression matrix passes:
  - H.264/H.265/AV1 where supported
  - NVENC/AMF/QSV/Software selection
  - system audio only / mic only / both
  - long recording A/V sync validation