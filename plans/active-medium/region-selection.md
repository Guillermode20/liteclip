# Plan: Region/Area Selection for Recording

## Status
Pending

## Priority
Medium

## Summary
Allow users to record a specific region of the screen instead of the full monitor. This is useful for recording specific application windows, game UI elements, or creating focused content.

## Current State
- DXGI Desktop Duplication captures the entire monitor
- GPU index selection allows choosing which monitor to capture
- No region selection or window-specific capture exists
- Resolution scaling is supported but always from full frame

## Implementation Steps

### 1. Region Configuration
- Add `Region` type to `VideoConfig`: `Full | Rect { x, y, width, height } | Window { process_name }`
- Support region definition via:
  - Manual coordinate input in settings
  - Interactive region picker overlay (drag to select)
  - Window selection dropdown (capture specific process window)

### 2. DXGI Capture Modification
- After acquiring full frame, crop to the specified region
- Perform crop on GPU (D3D11 texture subresource) to avoid CPU copy
- Update frame dimensions and stride for the cropped region
- Handle region changes without restarting the capture pipeline

### 3. Region Picker UI
- Create a transparent overlay window for interactive region selection
- Show current monitor with draggable rectangle
- Snap to window edges and common aspect ratios (16:9, 4:3, 1:1)
- Display region dimensions in real-time

### 4. Window Capture Mode
- Enumerate top-level windows with their bounds
- Filter to visible, non-minimized windows
- Track window position changes (move/resize) during recording
- Handle window close gracefully (stop recording or switch to full screen)

### 5. Configuration Persistence
- Save region settings per-monitor
- Remember last-used region for quick re-selection
- Support named region presets

## Files to Modify
- `crates/liteclip-core/src/config/config_mod/types.rs` -- Add region config
- `crates/liteclip-core/src/capture/dxgi/capture.rs` -- Add region cropping
- `crates/liteclip-core/src/capture/dxgi/texture.rs` -- GPU texture subregion
- `src/gui/` -- New `region_picker.rs` module
- `src/gui/settings.rs` -- Add region selection UI
- `src/platform/` -- Add window enumeration (Win32 APIs)

## Estimated Effort
Large (5-7 days)

## Dependencies
- None (uses existing DXGI and Win32 infrastructure)

## Risks
- GPU texture cropping requires careful D3D11 resource management
- Window tracking is fragile (windows can move, resize, or close unexpectedly)
- Region selection overlay must not be captured itself
- DPI scaling complicates coordinate calculations
