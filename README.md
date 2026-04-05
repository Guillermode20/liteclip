# LiteClip

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/Rust-1.94%2B-blue.svg)](https://www.rust-lang.org)

Lightweight Windows screen recorder that captures your best moments retroactively. Never miss an epic play again.

## Features

- **Always-On Replay Buffer** — Continuously records in RAM with zero disk writes until you save. Capture that clutch moment even if you weren't recording.

- **Hardware-Accelerated Encoding** — First-class support for NVIDIA NVENC, AMD AMF, and Intel QSV with D3D11 zero-copy frame transport. Falls back to software encoding automatically.

- **Smart Audio Processing** — AI-powered noise suppression (RNNoise), automatic loudness normalization, and Discord-style noise gate for clean voice capture.

- **Lossless Clip Editor** — Built-in gallery with timeline scrubbing, multi-snippet trimming without re-encoding, and export to target file size.

- **Game-Aware Organization** — Auto-detects fullscreen games and organizes clips by game name. No manual sorting needed.

- **Minimal Resource Footprint** — Rust-powered with lock-free ring buffers, async I/O, and proactive memory management. Runs quietly in the system tray.

## Quick Start

1. Download and run the MSI from [Releases](https://github.com/Guillermode20/liteclip-recorder/releases)
2. LiteClip starts automatically in your system tray
3. Hit `Ctrl + Shift + S` anytime to save the last 30 seconds as an MP4
4. Press `Ctrl + Shift + G` to open the gallery and browse your clips

## Screenshots

| Gallery | Settings | Video Editor |
|:--------|:---------|:-------------|
| ![Gallery](screenshots/gallery-image.png) | ![Settings](screenshots/settings-image.png) | ![Video Editor](screenshots/video-editor-image.png) |

## GPU Testing Needed

> **:warning: Contributor Help Wanted :warning:**
>
> LiteClip is developed and tested primarily on **AMD GPUs**. The **NVIDIA NVENC** and **Intel QSV** encoder paths have **never been tested on real hardware** by the maintainer. If you have an NVIDIA or Intel GPU, your help is critical — these paths may contain bugs that only surface on actual hardware.

| Encoder | Status | Tested on | What needs testing |
|---------|--------|-----------|--------------------|
| **AMD AMF** | :white_check_mark: Tested | RX 6000/7000 series (RDNA/RDNA2/RDNA3) | Working well — this is the reference implementation |
| **NVIDIA NVENC** | :x: **Needs contributors** | Never tested on real hardware | D3D11 shared device path, NVENC options (preset, tune, zerolatency), CBR/VBR/CQ rate control, forced IDR, b_ref_mode |
| **Intel QSV** | :x: **Needs contributors** | Never tested on real hardware | D3D11→QSV device derivation via `av_hwdevice_ctx_create_derived`, surface mapping via `av_hwframe_map`, oneVPL/Media SDK compatibility, iGPU and Arc dGPU |

### How to Help

1. **Build and run:** Follow the [build instructions](CONTRIBUTING.md#quick-start)
2. **Force your encoder:** In settings, select NVENC or QSV (not Auto)
3. **Record a clip:** Press `Ctrl + Shift + S` to save a 30-second clip
4. **Check logs:** Run with `$env:RUST_LOG = "debug,liteclip_core=trace"` and look for unexpected CPU fallback warnings or errors
5. **Report results:** Use the [Hardware Encoder Test Report](https://github.com/Guillermode20/liteclip-recorder/issues/new?template=hardware_encoder_test.yml) issue template

Even a simple "it works" or "it falls back to CPU" report is valuable. Include your GPU model, driver version, and log output.

See [CONTRIBUTING.md](CONTRIBUTING.md) for detailed testing checklists and per-vendor verification steps.

## Hotkeys

Global hotkeys work even while gaming:

| Action | Default |
|:---|:---|
| Save clip | `Ctrl + Shift + S` |
| Open gallery | `Ctrl + Shift + G` |
| Toggle recording | `Ctrl + Shift + R` |

## Configuration

Settings are stored at `%APPDATA%\liteclip\config.toml` and include:

- **Replay duration**: 5-300 seconds of buffer
- **Video quality**: Resolution, bitrate (1-150 Mbps), framerate (10-144 FPS)
- **Encoder**: Auto-detect hardware or force software
- **Audio**: Per-source volume, noise suppression toggle, mic selection
- **Hotkeys**: Customize all bindings
- **Storage**: Change save directory and file naming

See [CONTRIBUTING.md](CONTRIBUTING.md) for build instructions.

## FFmpeg DLL Requirements

For development or running from source, FFmpeg 6.0+ shared DLLs are required:

**Required DLLs:**
- `avcodec-61.dll`
- `avformat-61.dll`
- `avutil-59.dll`
- `swresample-5.dll`
- `swscale-8.dll`

**Setup:**
1. Download FFmpeg 6.0+ shared libraries from [gyan.dev](https://www.gyan.dev/ffmpeg/builds/) or [BtbN](https://github.com/BtbN/FFmpeg-Builds/releases)
2. Extract the `bin` folder contents to `ffmpeg_dev/sdk/bin/` in the project root
3. The build script automatically copies DLLs next to the executable

The MSI installer includes these DLLs pre-bundled, so end users don't need to set this up manually.

## AI Disclosure

This project was developed with assistance from AI coding tools, including code generation, refactoring suggestions, and documentation help. Human oversight and review was applied to all AI-assisted contributions.

## License

MIT License. See [LICENSE](LICENSE).
