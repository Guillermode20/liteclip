# AGENTS.md - LiteClip Replay

## Overview

LiteClip Replay is a lightweight Windows desktop screen recording application with a replay buffer feature. It allows users to continuously record their screen in a rolling buffer and save clips retroactively using a global hotkey.

## Project Details

| Attribute | Value |
|-----------|-------|
| **Name** | liteclip-replay |
| **Version** | 0.1.0 |
| **Language** | Rust (Edition 2021) |
| **Platform** | Windows (primary) |

## Tech Stack

| Category | Technology |
|----------|------------|
| **GUI Framework** | egui via eframe (immediate mode) |
| **Screen Capture** | Windows Graphics Capture API (via windows-capture crate) |
| **Video Encoding** | Windows Media Foundation H.264 (via windows-capture crate) |
| **Audio Capture** | Windows loopback audio (via windows-capture crate) |
| **Segment Concat** | Native binary MPEG-TS concatenation (no external tools) |
| **Global Hotkeys** | global-hotkey crate |
| **File Dialogs** | rfd (Rust File Dialogs) |

## Project Structure

```
.
├── Cargo.toml          # Package manifest and dependencies
├── Cargo.lock          # Dependency lock file
├── .gitignore          # Git ignore rules
├── todo.md             # Feature roadmap
└── src/
    ├── main.rs         # Entry point & hotkey management
    ├── gui.rs          # Medal-style UI implementation
    ├── recorder.rs     # FFmpeg recording logic
    └── settings.rs     # Settings enums & configuration
```

## Dependencies

```toml
[dependencies]
eframe = { version = "0.31", features = ["default_fonts", "glow"] }
rfd = "0.15"
global-hotkey = "0.6"
chrono = "0.4"
dirs = "6"
tempfile = "3"
```

## Build Commands

```powershell
# Build debug version
cargo build

# Build optimized release
cargo build --release

# Run the application
cargo run

# Check for errors without building
cargo check

# Format code
cargo fmt

# Run linter
cargo clippy

# Clean build artifacts
cargo clean
```

## Development Guidelines

### Code Style
- Follow standard Rust formatting (`cargo fmt`)
- Address clippy warnings before committing
- Use `unwrap()` sparingly; prefer proper error handling

### Architecture
- **main.rs**: Initializes the application, manages global hotkeys via `HotkeyWrapper`
- **gui.rs**: Contains `LiteClipApp` struct with all UI logic
- **Replay.rs**: Windows native capture management and segment handling
- **settings.rs**: Configuration enums and default values

### State Management
- Replay state is wrapped in `Arc<Mutex<Replay>>` for thread safety
- Global hotkeys communicate with the GUI via message passing
- Settings are persisted in memory (restarts reset to defaults)

### Window Configuration
- Default size: 340 x 320 pixels
- Min size: 300 x 260 pixels
- Max size: 500 x 500 pixels
- Always on top: Enabled

## Key Features

- Rolling buffer recording (configurable: 30s / 1m / 2m / 5m / 10m)
- Global hotkey activation (F8, F9, F10, Ctrl+Shift+S, or Alt+F9)
- Desktop audio capture
- Quality presets (Low/Medium/High/Ultra)
- Configurable framerate (15/30/60 FPS)
- Resolution scaling (Native/1080p/720p/480p)
- Medal-style compact overlay UI

## External Requirements

- Windows 10/11 (uses Windows Graphics Capture API and Media Foundation)
- No external tools required (FFmpeg is NOT needed)

## Release Build Optimization

The release profile is optimized for size and performance:

```toml
[profile.release]
codegen-units = 1
lto = "fat"
panic = "abort"
strip = true
```

## Common Tasks

### Adding a New Setting
1. Add the enum/field to `settings.rs`
2. Add UI controls in `gui.rs` (in the settings panel)
3. Apply the setting in `Replay.rs` if it affects recording
4. Update hotkey handling in `main.rs` if needed

### Adding a New Hotkey Preset
1. Add variant to `HotkeyPreset` enum in `settings.rs`
2. Add label in `label()` method
3. Add conversion in `create_hotkey()` function in `main.rs`

### Modifying FFmpeg Arguments
- Edit `get_ffmpeg_args()` in `recorder.rs`
- Test with different quality/framerate combinations
- Ensure output paths use proper escaping for Windows
