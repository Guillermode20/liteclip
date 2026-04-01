# Contributing to LiteClip

## Quick Start

```bash
git clone https://github.com/Guillermode20/liteclip-recorder.git
cd liteclip-recorder
cargo build --release --features ffmpeg
```

## Prerequisites

- Rust 1.94+ (see `rust-toolchain.toml`)
- FFmpeg 6.0+ shared libraries
- Windows SDK + Visual Studio Build Tools 2022

## Code Style

```bash
cargo fmt --check
cargo clippy -- -D warnings
cargo test
```

- Document all `pub` items
- Use `Result` over `unwrap`
- Add `# Safety` docs for `unsafe` blocks

## PR Process

1. Fork and branch from `master`
2. Add tests for new functionality
3. Run the checks above
4. Open a pull request

Commit format: `type(scope): description` (e.g., `fix(capture): handle DXGI access lost`)

## Debugging

```powershell
$env:RUST_LOG = "debug,liteclip_core=trace,wgpu=warn,naga=warn"
cargo run
```

### Common Issues

- **DXGI_ACCESS_LOST**: Expected on secure desktop (UAC, lock screen). Capture thread handles reacquisition — don't panic.
- **Hardware encoder fallback**: Check logs for unexpected CPU fallback. NVENC/AMF/QSV fall back to software when unavailable.
- **FFmpeg DLL not found**: Place `avcodec-*.dll`, `avformat-*.dll`, `avutil-*.dll`, `swscale-*.dll`, `swresample-*.dll`, `avfilter-*.dll` next to the executable.

## Hardware Encoders (NVENC / QSV / AMF)

**Maintainer note:** LiteClip is developed and tested primarily on AMD GPUs. The AMF encoder
path is the reference implementation. **Contributors with NVIDIA and Intel GPUs are strongly encouraged
to test, report issues, and submit improvements.**

### AMD AMF (Primary Tested)

- **Status:** Actively tested on RDNA/RDNA2/RDNA3 architectures
- **Key source:** `crates/liteclip-core/src/encode/ffmpeg/amf.rs`
- **Features:** D3D11 shared device, CBR/VBR/CQ rate control, quality presets (Performance/Balanced/Quality), VBAQ, pre-analysis, mini-GOP IDR insertion
- **Known working:** Radeon RX 6000/7000 series, Adrenalin drivers

### NVIDIA NVENC (Needs Testing)

- **Key source:** `crates/liteclip-core/src/encode/ffmpeg/nvenc.rs`
- **Features that need verification:**
  - D3D11 shared device transport (zero-copy frame path)
  - NVENC options: `preset`, `tune`, `delay=0`, `zerolatency=1`, `strict_gop=1`, `b_ref_mode=disabled`
  - Rate control: CBR, VBR, CQ modes with `cq` parameter
  - Forced IDR frames (`forced-idr=1`)
  - Bitrate/peak bitrate/buffer size configuration
- **Verification steps:**
  1. Set encoder to NVENC (or Auto with NVIDIA GPU present)
  2. Record a clip and confirm no CPU fallback in logs
  3. Verify output quality at various bitrates and rate control modes
  4. Test with different quality presets
- **Report:** GPU model, driver version, FFmpeg version, encoder output logs

### Intel QSV (Needs Testing)

- **Key source:** `crates/liteclip-core/src/encode/ffmpeg/qsv.rs`
- **Features that need verification:**
  - D3D11→QSV device derivation via `av_hwdevice_ctx_create_derived`
  - QSV surface mapping from D3D11 via `av_hwframe_map`
  - QSV frames context derivation from D3D11 frames context
  - Rate control: CBR, VBR modes with look_ahead disabled
  - QSV preset configuration
- **Verification steps:**
  1. Set encoder to QSV (or Auto with Intel GPU present)
  2. Record a clip and watch for derive/map errors in logs
  3. Confirm QSV surfaces are created and mapped successfully
  4. Test on both iGPU and dGPU (Arc) configurations
- **Report:** GPU model (iGPU/Arc), driver version, oneVPL/Media SDK version, FFmpeg version, encoder output logs

### Encoder Auto-Detection

- **Source:** `crates/liteclip-core/src/encode/encoder_mod/functions.rs`
- The `detect_available_encoder()` function probes for hardware encoders in priority order
- Falls back to software (libx264/libx265) when no hardware encoder is available
- Test auto-detection on systems with multiple GPUs (e.g., Intel iGPU + NVIDIA dGPU)

### Reporting Encoder Issues

When reporting encoder-related issues, include:

```
- GPU model: (e.g., RTX 4070, RX 7800 XT, Arc A770, UHD 770)
- Driver version: (e.g., 551.86, 24.2.1, 31.0.101.5186)
- FFmpeg version: (e.g., 6.1.1 shared)
- Encoder selected: (Auto / NVENC / QSV / AMF / Software)
- Codec: (H.264 / H.265)
- Rate control: (CBR / VBR / CQ)
- Log output: (RUST_LOG=debug,liteclip_core=trace)
- Issue description: (fallback to CPU, crash, artifacts, etc.)
```

PRs that change encoder behavior should include GPU model, driver notes, and relevant `tracing` output. Key source locations:

- Hub: `crates/liteclip-core/src/encode/ffmpeg/mod.rs`
- NVENC: `crates/liteclip-core/src/encode/ffmpeg/nvenc.rs`
- QSV: `crates/liteclip-core/src/encode/ffmpeg/qsv.rs`
- AMF: `crates/liteclip-core/src/encode/ffmpeg/amf.rs`
- Auto-detect: `crates/liteclip-core/src/encode/encoder_mod/functions.rs`

See [AGENTS.md](AGENTS.md) for full architecture details, threading model, and memory management patterns.

## Questions?

Open an issue for discussion.