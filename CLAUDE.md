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
│  DXGI Capture   │ --> │  HW Encoder      │ --> │  Replay Buffer  │
│  (Desktop Dup)  │     │  (FFmpeg subprocess)│     │  (Ring Buffer)  │
└─────────────────┘     └──────────────────┘     └─────────────────┘
                                                        │
┌─────────────────┐                                     │
│  WASAPI Audio   │ ─────────────────────────────────> │
│  (System + Mic) │                                     │
└─────────────────┘                                     │
                                                        v
                                                 ┌─────────────┐
                                                 │  MP4 Muxer  │
                                                 │  (on save)  │
                                                 └─────────────┘
```

### Key Modules

- **`app.rs`**: Central `AppState` managing `RecordingPipeline` and `SharedReplayBuffer`. All recording start/stop logic lives here.

- **`capture/`**: Screen and audio capture
  - `dxgi/`: DXGI Desktop Duplication for GPU-side frame capture
  - `audio/`: WASAPI audio capture with mixer for system audio + microphone

- **`encode/`**: Video encoding via FFmpeg subprocess
  - `hw_encoder/`: Hardware encoders (NVENC, AMF, QSV) - spawns ffmpeg.exe with gdigrab
  - `sw_encoder/`: Software encoder fallback
  - Two modes: "pull mode" (FFmpeg grabs directly) vs "capture mode" (app captures, pushes frames to encoder)

- **`buffer/ring/`**: In-memory ring buffer holding encoded packets. Uses `Bytes` for zero-copy reference counting. Key method: `snapshot()` for clip saving.

- **`clip/muxer/`**: MP4 muxing when saving clips. Handles H.264/H.265/AV1 video and AAC audio.

- **`platform/`**: Windows-specific integration
  - Hidden HWND for global hotkey registration
  - System tray icon with menu
  - Windows message loop thread

- **`config/`**: TOML configuration loaded from `%APPDATA%/liteclip-replay/config.toml`

### Recording Modes

1. **Hardware Pull Mode**: FFmpeg subprocess uses gdigrab to capture directly. Used when hardware encoder (NVENC/AMF/QSV) is available and `use_cpu_readback=false`.

2. **Capture Mode**: App uses DXGI Desktop Duplication to capture frames, performs CPU readback, pushes to encoder. Used for software encoding or when CPU readback is enabled.

### Threading Model

- Main thread: Tokio async runtime handling events, state management
- Platform thread: Windows message loop for hotkeys and tray
- Encoder thread: Spawned by `spawn_encoder()`, consumes frames and produces packets
- Capture thread: DXGI capture loop (when in capture mode)
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
- **New encoder**: Add trait implementation in `encode/hw_encoder/`, add detection logic in `app.rs`
- **New config option**: Add to config types in `config/config_mod/types.rs`, update `Config::validate()`