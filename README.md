# LiteClip Replay

A lightweight, high-performance Windows screen recorder with replay buffer functionality. Capture your gameplay or desktop with minimal overhead and save clips retroactively.

## Features

- **Replay Buffer**: Continuously record in the background; save the last N seconds on demand
- **Hardware Encoding**: NVENC (NVIDIA), AMF (AMD), QSV (Intel), and software fallback
- **Low Latency**: DXGI Desktop Duplication for GPU-accelerated capture
- **Audio Capture**: WASAPI-based system audio and microphone recording
- **System Tray**: Minimal UI with tray icon controls
- **Hotkeys**: Global hotkeys for clip saving, recording toggle, and more
- **Auto-Start**: Optional Windows startup integration
- **Game Detection**: Automatic detection of running games for organized clip storage

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                         LiteClip Replay                          │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌──────────┐    ┌──────────────┐    ┌───────────────────────┐  │
│  │  DXGI    │    │  Recording   │    │   Replay Buffer       │  │
│  │  Capture │───▶│   Pipeline   │───▶│   (Lock-free Ring)    │  │
│  │  (GPU)   │    │              │    │                       │  │
│  └──────────┘    │ ┌──────────┐ │    └───────────┬───────────┘  │
│  ┌──────────┐    │ │ Encoder  │ │                │              │
│  │  WASAPI  │    │ │ NVENC/   │ │                ▼              │
│  │  Audio   │───▶│ │ AMF/QSV/ │ │    ┌───────────────────────┐  │
│  │          │    │ │ SW       │ │    │    Clip Saver         │  │
│  └──────────┘    │ └──────────┘ │    │   (FFmpeg Muxer)      │  │
│                  └──────────────┘    └───────────────────────┘  │
│                                                                  │
├─────────────────────────────────────────────────────────────────┤
│        Platform Layer (Hotkeys, Tray, Notifications)            │
├─────────────────────────────────────────────────────────────────┤
│        GUI Layer (Settings, Gallery, Clip Overlay)              │
└─────────────────────────────────────────────────────────────────┘
```

### Module Overview

| Module | Description |
|--------|-------------|
| `app` | Application state and recording pipeline coordination |
| `buffer` | Lock-free ring buffer for replay storage |
| `capture` | DXGI screen capture and WASAPI audio capture |
| `encode` | Video encoding (NVENC/AMF/QSV/software) |
| `clip` | Clip saving and muxing |
| `config` | Configuration management |
| `platform` | Windows integration (hotkeys, tray, notifications) |
| `gui` | Settings and gallery UI (egui) |
| `output` | Output file handling and thumbnails |
| `detection` | Running game detection |

## Installation

### Pre-built Releases

Download the latest release from the [Releases](https://github.com/your-repo/liteclip-recorder/releases) page.

### Build from Source

**Prerequisites:**
- Rust 1.70 or later
- FFmpeg 6.0+ (shared libraries)
- Windows SDK

```bash
# Clone the repository
git clone https://github.com/your-repo/liteclip-recorder.git
cd liteclip-recorder

# Build in release mode
cargo build --release --features ffmpeg
```

The binary will be at `target/release/liteclip-replay.exe`.

## Usage

### Quick Start

1. Run `liteclip-replay.exe`
2. The app runs in the system tray
3. Use hotkeys or right-click the tray icon for actions

### Hotkeys

| Action | Default Hotkey | Description |
|--------|---------------|-------------|
| Save Clip | `Ctrl+Shift+S` | Save the current replay buffer to disk |
| Toggle Recording | `Ctrl+Shift+R` | Start/stop the recording pipeline |
| Screenshot | `Ctrl+Shift+X` | Capture a screenshot (coming soon) |
| Open Gallery | `Ctrl+Shift+G` | Open the clip gallery |

Hotkeys can be customized in Settings.

### Tray Menu

Right-click the tray icon to access:
- **Save Clip**: Save current replay buffer
- **Open Settings**: Configure recording options
- **Open Gallery**: Browse saved clips
- **Restart**: Restart the application
- **Exit**: Close the application

## Configuration

Configuration is stored at `%APPDATA%\liteclip-replay\config.toml`.

### Key Settings

```toml
[general]
replay_duration_secs = 60        # Replay buffer length
auto_start_with_windows = false  # Launch on Windows startup
start_minimised = false          # Start hidden in tray
notifications = true             # Show desktop notifications

[video]
framerate = 60                   # Target FPS
bitrate_mbps = 10                # Video bitrate
encoder = "nvenc"                # Encoder: nvenc, amf, qsv, software
codec = "hevc"                   # Codec: h264, hevc
quality_preset = "balanced"      # Encoder preset

[audio]
capture_system = true            # Capture system audio
capture_mic = false              # Capture microphone
system_volume = 100              # System audio volume %
mic_volume = 100                 # Microphone volume %

[hotkeys]
save_clip = "Ctrl+Shift+S"
toggle_recording = "Ctrl+Shift+R"
```

### Encoder Selection

| Encoder | Requirements | Notes |
|---------|-------------|-------|
| `nvenc` | NVIDIA GPU (GTX 600+) | Best quality/performance |
| `amf` | AMD GPU | Good performance |
| `qsv` | Intel iGPU | Moderate performance |
| `software` | CPU only | Slow but universal |

### Video Codecs

| Codec | Description | Compatibility |
|-------|-------------|---------------|
| `h264` | H.264/AVC | Universal support |
| `hevc` | H.265/HEVC | Better compression, newer devices |

## Project Structure

```
liteclip-recorder/
├── src/
│   ├── main.rs           # Application entry point
│   ├── lib.rs            # Crate root and public API
│   ├── app/              # Application state and pipeline
│   │   ├── state.rs      # Core state management
│   │   ├── clip.rs       # Clip saving logic
│   │   └── pipeline/     # Recording pipeline
│   ├── buffer/           # Replay buffer implementation
│   │   └── ring/         # Lock-free ring buffer
│   ├── capture/          # Screen and audio capture
│   │   ├── dxgi/         # DXGI desktop capture
│   │   └── audio/        # WASAPI audio capture
│   ├── encode/           # Video encoding
│   │   └── ffmpeg/       # FFmpeg integration
│   ├── platform/         # Windows platform integration
│   │   ├── hotkeys.rs    # Global hotkeys
│   │   └── tray.rs       # System tray
│   ├── gui/              # User interfaces
│   ├── config/           # Configuration system
│   ├── output/           # File output handling
│   └── detection/        # Game detection
├── installer/            # WiX installer project
└── Cargo.toml            # Package manifest
```

## Development

See [CONTRIBUTING.md](CONTRIBUTING.md) for development guidelines.

### Running Tests

```bash
cargo test
```

### Code Quality

```bash
cargo clippy -- -D warnings
cargo fmt --check
```

## License

MIT License. See [LICENSE](LICENSE) for details.

## Acknowledgments

- [FFmpeg](https://ffmpeg.org/) for encoding and muxing
- [egui](https://github.com/emilk/egui) for UI framework
- [windows-rs](https://github.com/microsoft/windows-rs) for Windows API bindings