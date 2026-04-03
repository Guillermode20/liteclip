# Plan: Encoder-Specific Advanced Options

## Status
Pending

## Priority
Low

## Summary
Expose encoder-specific advanced options for power users. Currently the config exposes encoder type, quality preset, rate control, and CQ value, but not hardware-specific options like NVENC's `lookahead`, `b-frames`, `spatial_aq`, or QSV's `low_power` mode.

## Current State
- Generic encoder options: type, quality preset, rate control, CQ value
- No encoder-specific advanced options exposed
- NVENC, AMF, and QSV each have unique options that are hardcoded
- Power users cannot fine-tune encoder behavior

## Implementation Steps

### 1. Advanced Options Schema
- Define encoder-specific option structs:
  - **NVENC**: `lookahead: u32`, `b_frames: u32`, `spatial_aq: bool`, `temporal_aq: bool`, `zerolatency: bool`
  - **AMF**: `enforce_hrd: bool`, `vbaq: bool`, `header_insertion_mode: bool`
  - **QSV**: `low_power: bool`, `lookahead_depth: u32`, `adaptive_i: bool`, `adaptive_b: bool`
  - **Software**: `preset: String`, `tune: String`, `profile: String`

### 2. Configuration
- Add `advanced_encoder_options` to `VideoConfig` as a nested struct
- Serialize as encoder-specific TOML sections:
  ```toml
  [video.advanced.nvenc]
  spatial_aq = true
  lookahead = 32

  [video.advanced.qsv]
  low_power = true
  ```

### 3. FFmpeg Integration
- Map advanced options to FFmpeg `-x265-params`, `-nvenc-params`, etc.
- Validate options against encoder capabilities at init time
- Log warnings for unsupported options on current hardware

### 4. GUI
- Add "Advanced Encoder Options" collapsible section in Video tab
- Show only options relevant to the selected encoder
- Tooltips explaining each option's effect
- Reset to defaults button

### 5. Validation
- Validate option ranges and compatibility
- Some options are mutually exclusive (e.g., `zerolatency` and `lookahead`)
- Warn users about options that may impact performance

## Files to Modify
- `crates/liteclip-core/src/config/config_mod/types.rs` — Advanced encoder options struct
- `crates/liteclip-core/src/encode/ffmpeg/options.rs` — Apply advanced options per encoder
- `crates/liteclip-core/src/encode/ffmpeg/nvenc.rs` — NVENC-specific options
- `crates/liteclip-core/src/encode/ffmpeg/amf.rs` — AMF-specific options
- `crates/liteclip-core/src/encode/ffmpeg/qsv.rs` — QSV-specific options
- `src/gui/settings.rs` — Advanced encoder options panel

## Estimated Effort
Medium (3-5 days)

## Dependencies
- None

## Risks
- Option names and availability vary by FFmpeg and driver version
- Invalid options may cause encoder initialization to fail silently
- GUI complexity increases with each encoder's unique options
