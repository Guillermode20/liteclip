# LiteClip Replay

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/Rust-1.74%2B-blue.svg)](https://www.rust-lang.org)

Lightweight Windows screen recorder with background replay buffer. Save clips on demand with `Ctrl + Shift + S`.

## Features

- **Background recording** - Continuous buffer, no disk writes until you save
- **GPU encoding** - NVIDIA NVENC, AMD AMF, Intel QSV support
- **Gallery & trim** - Browse, trim, and export clips without re-encoding
- **System tray app** - Runs quietly, minimal resource usage

## Download

Get the latest installer from [Releases](https://github.com/Guillermode20/liteclip-recorder/releases).

## Hotkeys

| Action | Default |
|:---|:---|
| Save clip | `Ctrl + Shift + S` |
| Open gallery | `Ctrl + Shift + G` |
| Toggle recording | `Ctrl + Shift + R` |

## Configuration

Settings stored at `%APPDATA%\liteclip-replay\config.toml`. See [CONTRIBUTING.md](CONTRIBUTING.md) for build instructions.

## License

MIT License. See [LICENSE](LICENSE).
