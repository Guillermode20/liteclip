# Plan: Cursor Visibility Toggle

## Status
Pending

## Priority
Medium

## Summary
Add a configuration option to show or hide the mouse cursor in recordings. DXGI Desktop Duplication always captures the cursor when visible. Many screen recorders offer a toggle for this.

## Current State
- No configuration option exists for cursor visibility
- DXGI captures the cursor by default via `IDXGIOutput1::DuplicateOutput`
- Users recording gameplay or presentations often want to hide the cursor
- Tutorial creators need the cursor visible

## Implementation Steps

### 1. Configuration
- Add `show_cursor: bool` to `VideoConfig` (default: `true` for backward compatibility)
- Serialize/deserialize in TOML config

### 2. DXGI Capture Modification
- Use `IDXGIOutput5::DuplicateOutput1` with `DXGI_OUTDUPL_FLAG_CURSOR` flag to control cursor capture
- When `show_cursor = false`: omit the flag so cursor is not composited into the frame
- When `show_cursor = true`: include the flag (current behavior)

### 3. GUI
- Add checkbox to the Video settings tab: "Show cursor in recordings"
- Place near resolution/framerate settings

### 4. Pipeline Restart
- Cursor visibility change requires reinitializing the duplication output
- Handle gracefully during pipeline restart on config change

## Files to Modify
- `crates/liteclip-core/src/config/config_mod/types.rs` — Add `show_cursor: bool` to VideoConfig
- `crates/liteclip-core/src/capture/dxgi/capture.rs` — Pass cursor flag to DuplicateOutput
- `src/gui/settings.rs` — Add checkbox in Video tab

## Estimated Effort
Small (1-2 days)

## Dependencies
- None

## Risks
- `IDXGIOutput5::DuplicateOutput1` requires Windows 10 Anniversary Update or later (widely available)
- Changing cursor visibility requires reinitializing the capture output
