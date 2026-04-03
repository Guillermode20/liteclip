# Plan: H.264 and AV1 Encoder Support

## Status
Pending

## Priority
High

## Summary
Currently LiteClip only supports HEVC (H.265) encoding. Add support for H.264 (AVC) and AV1 codecs to improve compatibility with editing software, streaming platforms, and older hardware.

## Current State
- All encoding is hardcoded to HEVC/H.265
- FFmpeg backend infrastructure supports multiple codecs
- Encoder auto-detection only probes for HEVC-capable encoders
- Configuration has no codec selection option

## Implementation Steps

### 1. Codec Configuration
- Add `codec` field to `VideoConfig`: `Hevc | H264 | Av1`
- Update config serialization/deserialization
- Add codec selector to the Video settings tab
- Maintain HEVC as the default for backward compatibility

### 2. FFmpeg Encoder Updates
- **NVENC**: Add `h264_nvenc` and `av1_nvenc` (RTX 40-series+) codec support
- **AMF**: Add `h264_amf` and `av1_amf` (RDNA2+) codec support
- **QSV**: Add `h264_qsv` and `av1_qsv` (11th gen+) codec support
- **Software**: Add `libx264` and `libsvtav1`/`libaom-av1` codec support
- Update encoder options per codec (not all codecs support all rate control modes)

### 3. Encoder Auto-Detection
- Update probe logic to check for codec availability per encoder
- Fallback matrix: if AV1 not supported on hardware, fall back to H.264, then software
- Log clear messages about codec availability

### 4. MP4 Muxer Compatibility
- Verify MP4 container supports all three codecs (it does)
- Update any codec-specific muxing parameters

### 5. Gallery/Export
- Ensure video probing correctly identifies all three codecs
- Update export pipeline to respect source codec or allow transcode

## Files to Modify
- `crates/liteclip-core/src/config/config_mod/types.rs` — Add codec enum and config field
- `crates/liteclip-core/src/encode/ffmpeg/nvenc.rs` — Add H.264/AV1 codec names
- `crates/liteclip-core/src/encode/ffmpeg/amf.rs` — Add H.264/AV1 codec names
- `crates/liteclip-core/src/encode/ffmpeg/qsv.rs` — Add H.264/AV1 codec names
- `crates/liteclip-core/src/encode/ffmpeg/software.rs` — Add libx264/AV1 software encoders
- `crates/liteclip-core/src/encode/encoder_mod/functions.rs` — Update auto-detection
- `crates/liteclip-core/src/encode/ffmpeg/mod.rs` — Codec-agnostic options
- `src/gui/settings.rs` — Add codec selector UI
- `src/gui/gallery.rs` — Update export codec handling

## Estimated Effort
Large (5-8 days)

## Dependencies
- FFmpeg must be built with appropriate codec support (already the case for standard builds)
- AV1 hardware encoding requires newer GPUs (RTX 40-series, RDNA2+, Intel Arc)

## Risks
- AV1 encoder availability varies significantly by hardware generation
- H.264 may have different quality/bitrate characteristics requiring preset adjustments
- Testing matrix grows: 3 codecs x 4 encoder backends = 12 combinations
