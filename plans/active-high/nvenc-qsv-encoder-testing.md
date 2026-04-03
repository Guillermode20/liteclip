# Plan: NVENC and QSV Encoder Testing & Verification

## Status
Pending

## Priority
High

## Summary
NVENC (NVIDIA) and QSV (Intel) hardware encoders are fully implemented but untested on real hardware. The maintainer only has an AMD GPU. This plan covers systematic testing, bug identification, and verification of these encoder paths.

## Current State
- NVENC implementation in `encode/ffmpeg/nvenc.rs` -- complete but untested
- QSV implementation in `encode/ffmpeg/qsv.rs` -- complete but untested
- AMF (AMD) is the primary tested reference implementation
- Auto-detection probes NVENC -> AMF -> QSV -> Software
- No CI GPU matrix exists; all hardware testing is manual

## Implementation Steps

### 1. Test Plan Creation
- Create a test checklist for each encoder covering:
  - Encoder initialization and device creation
  - D3D11 zero-copy frame input
  - All rate control modes (CBR, VBR, CQ)
  - All quality presets (Performance, Balanced, Quality)
  - Various resolutions (720p, 1080p, 1440p, 4K)
  - Various framerates (30, 60, 120, 144)
  - Bitrate range (1-150 Mbps)
  - Keyframe interval configuration
  - Static scene (duplicate frame optimization)
  - Dynamic scene (fast motion)

### 2. Diagnostic Logging
- Add detailed trace-level logging to NVENC and QSV init paths
- Log codec parameters, profile, level, and preset used
- Log any warnings or fallbacks during encoding
- Add encoder-specific error messages with actionable guidance

### 3. Automated Smoke Tests
- Create integration tests that attempt encoder initialization
- Tests should gracefully skip if hardware is not available
- Log results to a structured format for community reporting

### 4. Community Testing Program
- Document testing instructions in CONTRIBUTING.md
- Provide a test binary with verbose logging
- Create a GitHub issue template for encoder bug reports
- Collect test results from community members

### 5. Bug Fixes
- Address any issues found during testing
- Update encoder options for hardware-specific quirks
- Add workarounds for known driver bugs

## Files to Modify
- `crates/liteclip-core/src/encode/ffmpeg/nvenc.rs` — Add diagnostic logging, fix bugs
- `crates/liteclip-core/src/encode/ffmpeg/qsv.rs` — Add diagnostic logging, fix bugs
- `crates/liteclip-core/tests/` — Add encoder smoke tests
- `CONTRIBUTING.md` — Add community testing instructions

## Estimated Effort
Medium (3-5 days for logging/tests + community testing time)

## Dependencies
- Access to NVIDIA and Intel GPU hardware for testing
- Community participation for broader coverage

## Risks
- Hardware-specific bugs may only appear on certain GPU generations
- Driver version differences can cause inconsistent behavior
- Cannot fully test without physical hardware access
