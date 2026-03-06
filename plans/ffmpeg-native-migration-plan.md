# FFmpeg CLI -> Native FFmpeg Migration Plan

## Objective
Complete the migration from spawning `ffmpeg` as a subprocess (CLI mode) to using native FFmpeg APIs through `ffmpeg-next` for recording + clip muxing, while preserving feature parity (hardware encoding behavior, A/V sync, and output quality).

## Completed ✅

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

### 4) Legacy hardware pull mode is disabled
- `should_use_hardware_pull_mode()` hard-returns `false`.
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

### 8) Dead code removed
- Deleted `hw_encoder` module (CLI subprocess encoders).
- Deleted `cpu_readback.rs` (unused stub).
- Deleted `frame_writer.rs` (CLI stdin writer).
- Updated module documentation in `encode/mod.rs`.

### 9) Documentation updated
- Updated `CLAUDE.md` to reflect native FFmpeg architecture.

### 10) Installer cleaned up
- Removed `cmpFfmpegExe` component (CLI executable no longer shipped).
- Updated `build.ps1` to check for FFmpeg DLLs instead of CLI exe.
- Updated `installer/README.md` to reflect native FFmpeg requirements.
- Installer now only harvests FFmpeg DLLs from `ffmpeg/bin/`.

## Remaining Work (Optional)

### P2 - Config cleanup
- `use_cpu_readback` and `output_index` fields may reflect old architecture.
  - Evidence: @src/encode/encoder_mod/types.rs#106-109

**Plan**
1. Review if these fields are still meaningful.
2. Rename or remove as appropriate.

## Definition of Done
- [x] No runtime `Command::new` calls for FFmpeg **encoding** paths.
- [x] Native muxer writes both video and audio tracks correctly.
- [x] Hardware encoder detection is native-only.
- [x] `faststart` option is implemented.
- [x] `hw_encoder` module deleted.
- [x] Documentation updated.
- [x] Installer no longer ships FFmpeg CLI executable.
- [ ] Regression matrix passes:
  - H.264/H.265/AV1 where supported
  - NVENC/AMF/QSV/Software selection
  - system audio only / mic only / both
  - long recording A/V sync validation