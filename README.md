# LiteClip Replay

LiteClip Replay is a lightweight Windows screen recorder that continuously records in the background and lets you save short clips on demand.

---

## Workflow: Clip → Trim → Export → Share

### Clip
While you play or work, LiteClip records into a rolling buffer. When something worth saving happens, press the hotkey to capture the last few seconds.

### Trim
Open the **Gallery** and select the clip you just created. Use the built-in editor to trim the beginning and end without re-encoding or waiting.

### Export
Export the trimmed clip in an optimized format. LiteClip automatically balances file size and quality so the result is ready for online sharing.

### Share
After exporting, your clip is ready to upload or drag into chat apps, social media, or streaming platforms.

---

## Gallery Features

The Gallery provides a central place to manage your recordings:

- **Instant previews** while hovering over clips
- **Trim tool** for selecting start and end frames
- **Automatic organization** by game or application
- **Recording status** and storage usage indicators

---

## Performance & Behavior

LiteClip is designed to run with minimal impact:

- Uses GPU hardware encoding (NVIDIA NVENC, AMD AMF, Intel QSV) when available
- Keeps recordings in a memory buffer (no disk writes until you save)
- Supports automatic startup and game detection

---

## Hotkeys

| Action | Default |
|:---|:---|
| Save clip | `Ctrl + Shift + S` |
| Open gallery | `Ctrl + Shift + G` |
| Toggle recording | `Ctrl + Shift + R` |

Hotkeys can be adjusted in Settings.

---

## Getting Started

### Download
Get the latest installer from the [Releases](https://github.com/your-repo/liteclip-recorder/releases) page.

### Run
Launch LiteClip and let it run in the system tray. When you want to save a moment, use the save clip hotkey.

### Configure (optional)
Right-click the tray icon and choose **Settings** to adjust clip length, encoder options, and audio sources.

---

## Configuration File

Settings are stored at `%APPDATA%\liteclip-replay\config.toml`.

### Example

```toml
[general]
replay_duration_secs = 60
auto_start_with_windows = false
start_minimised = false
notifications = true

[video]
framerate = 60
bitrate_mbps = 10
encoder = "nvenc"
codec = "hevc"
quality_preset = "balanced"

[audio]
capture_system = true
capture_mic = false

[hotkeys]
save_clip = "Ctrl+Shift+S"
toggle_recording = "Ctrl+Shift+R"
```

---

## Build & Development

See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

```bash
cargo test
cargo clippy -- -D warnings
cargo fmt --check
```

---

## License

MIT License. See [LICENSE](LICENSE).