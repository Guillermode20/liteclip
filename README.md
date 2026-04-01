# LiteClip

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/Rust-1.94%2B-blue.svg)](https://www.rust-lang.org)

Lightweight Windows screen recorder with background replay buffer. Save clips on demand with `Ctrl + Shift + S`.

## Features

- **Background recording** - Continuous buffer, no disk writes until you save
- **GPU encoding** - NVIDIA NVENC, AMD AMF, Intel QSV support
- **Gallery & trim** - Browse, trim, and export clips without re-encoding
- **System tray app** - Runs quietly, minimal resource usage

## GPU Testing Needed

**This project relies on hardware-accelerated encoding across all major GPU vendors, but the
maintainer only has an AMD GPU for testing.** If you have an NVIDIA or Intel GPU, your help is
critical to ensure these encoder paths work correctly.

| Encoder | Status | What needs testing |
|---------|--------|--------------------|
| **AMD AMF** | Primary tested | Working well on RDNA/RDNA2/RDNA3 |
| **NVIDIA NVENC** | Needs testing | D3D11 shared device path, NVENC options, CBR/VBR/CQ rate control, forced IDR, zero-latency preset |
| **Intel QSV** | Needs testing | D3D11→QSV device derivation, surface mapping, oneVPL/Media SDK compatibility |

See [CONTRIBUTING.md](CONTRIBUTING.md) for detailed testing checklists and how to report results.

## Download

Get the latest installer from [Releases](https://github.com/Guillermode20/liteclip-recorder/releases).

## Hotkeys

| Action | Default |
|:---|:---|
| Save clip | `Ctrl + Shift + S` |
| Open gallery | `Ctrl + Shift + G` |
| Toggle recording | `Ctrl + Shift + R` |

## Configuration

Settings stored at `%APPDATA%\liteclip\config.toml`. See [CONTRIBUTING.md](CONTRIBUTING.md) for build instructions.

## License

MIT License. See [LICENSE](LICENSE).
