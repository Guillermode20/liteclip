# Plan: Screenshot Hotkey Implementation

## Status
Pending

## Priority
High

## Summary
Implement the screenshot hotkey feature. The hotkey configuration and parsing infrastructure already exists in `HotkeyConfig` (`save_clip`, `toggle_recording`, `screenshot`, `open_gallery`), but the screenshot handler in `main.rs` currently logs "not implemented".

## Current State
- `HotkeyConfig` has a `screenshot` field with a default keybinding
- Hotkey parsing and registration pipeline is fully functional
- Handler in `main.rs:437-438` logs a warning and returns without action
- DXGI capture pipeline already produces frames that could be snapshot

## Implementation Steps

### 1. Capture Pipeline Extension
- Add a `capture_screenshot()` method to the capture backend trait
- For DXGI: grab the current frame and read it back to CPU (NV12 or BGRA)
- Reuse existing D3D11 device/context infrastructure

### 2. Image Encoding
- Convert captured frame to a standard image format (PNG or JPEG)
- Use the existing `image` crate dependency for encoding
- Consider using `swscale` from FFmpeg for NV12 -> RGB conversion if already available

### 3. File Output
- Save to the configured `save_directory` with timestamped naming
- Support configurable format (PNG for lossless, JPEG for smaller size)
- Add to gallery index so it appears in the browser

### 4. GUI Integration
- Add screenshot format and quality settings to the Settings window
- Show toast notification on successful screenshot save
- Display screenshots in gallery with appropriate thumbnail

### 5. Hotkey Handler Wiring
- Connect the hotkey event in `main.rs` to the new screenshot pipeline
- Debounce to prevent accidental double-screenshots

## Files to Modify
- `src/main.rs` — Wire up the screenshot hotkey handler
- `crates/liteclip-core/src/capture/` — Add screenshot capture method
- `crates/liteclip-core/src/config/config_mod/types.rs` — Add screenshot format/quality config
- `crates/liteclip-core/src/output/` — Add screenshot save logic
- `src/gui/settings.rs` — Add screenshot settings tab/section
- `src/gui/gallery.rs` — Support displaying screenshots alongside video clips

## Estimated Effort
Medium (3-5 days)

## Dependencies
- None (uses existing capture pipeline and dependencies)

## Risks
- Screenshot capture during active recording should not interfere with the replay buffer
- GPU readback for screenshots could cause a frame drop if not careful
