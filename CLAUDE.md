# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build Commands

```bash
cargo build              # Debug build
cargo build --release    # Release build (LTO, stripped)
cargo run                # Run the application
cargo test               # Run tests
```

## Architecture Overview

LiteClip Replay is a Windows-only screen recorder with a replay buffer (similar to NVIDIA ShadowPlay or OBS replay buffer). It captures desktop frames and audio continuously, keeping the last N seconds in memory. When triggered, it saves the buffer to an MP4 file.

### Core Pipeline

```
┌─────────────────┐     ┌──────────────────┐     ┌─────────────────┐
│  DXGI Capture   │ --> │  Native Encoder  │ --> │  Replay Buffer  │
│  (Desktop Dup)  │     │  (ffmpeg-next)   │     │  (Ring Buffer)  │
└─────────────────┘     └──────────────────┘     └─────────────────┘
                                                        │
┌─────────────────┐                                     │
│  WASAPI Audio   │ ─────────────────────────────────> │
│  (System + Mic) │                                     │
└─────────────────┘                                     │
                                                        v
                                                 ┌─────────────┐
                                                 │  MP4 Muxer  │
                                                 │  (ffmpeg-next)│
                                                 └─────────────┘
```

### Key Modules

- **`app.rs`**: Central `AppState` managing `RecordingPipeline` and `SharedReplayBuffer`. All recording start/stop logic lives here.

- **`capture/`**: Screen and audio capture
  - `dxgi/`: DXGI Desktop Duplication for GPU-side frame capture
  - `audio/`: WASAPI audio capture with mixer for system audio + microphone

- **`encode/`**: Video encoding using native FFmpeg APIs (`ffmpeg-next`)
  - `ffmpeg_encoder.rs`: Native encoder supporting NVENC, AMF, QSV, and software (libx264/libx265/libaom-av1)
  - `encoder_mod/`: Encoder trait, configuration, and spawning logic
  - `sw_encoder/`: JPEG fallback encoder for non-FFmpeg builds

- **`buffer/ring/`**: In-memory ring buffer holding encoded packets. Uses `Bytes` for zero-copy reference counting. Key method: `snapshot()` for clip saving.

- **`clip/muxer/`**: MP4 muxing using native FFmpeg APIs. Handles H.264/H.265/AV1 video and AAC audio with faststart support.

- **`platform/`**: Windows-specific integration
  - Hidden HWND for global hotkey registration
  - System tray icon with menu
  - Windows message loop thread

- **`config/`**: TOML configuration loaded from `%APPDATA%/liteclip-replay/config.toml`

### Recording Pipeline

1. **DXGI Capture**: Desktop Duplication API captures frames from GPU
2. **CPU Readback**: Frames are read back to system memory as BGRA
3. **Native Encoding**: `FfmpegEncoder` uses `ffmpeg-next` to encode frames (hardware or software)
4. **Ring Buffer**: Encoded packets are stored with timestamp metadata
5. **Clip Save**: `FfmpegMuxer` writes MP4 with interleaved audio/video

### Threading Model

- Main thread: Tokio async runtime handling events, state management
- Platform thread: Windows message loop for hotkeys and tray
- Encoder thread: Spawned by `spawn_encoder_with_receiver()`, consumes frames and produces packets
- Capture thread: DXGI capture loop
- Audio threads: WASAPI capture threads

### Configuration

Config file location: `%APPDATA%/liteclip-replay/config.toml`

Key settings:
- `general.replay_duration_secs`: Buffer duration
- `video.encoder`: Auto/Nvenc/Amf/Qsv/Software
- `video.codec`: H264/H265/Av1
- `hotkeys.save_clip`: Hotkey string like "Alt+F9"

### Adding New Features

- **New hotkey action**: Add to `HotkeyAction` enum in `platform/mod.rs`, implement handling in `main.rs` event loop
- **New encoder**: Add configuration in `EncoderConfig`, implement encoder options in `ffmpeg_encoder.rs`
- **New config option**: Add to config types in `config/config_mod/types.rs`, update `Config::validate()`