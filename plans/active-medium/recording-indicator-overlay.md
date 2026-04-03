# Plan: Recording Indicator Overlay

## Status
Pending

## Priority
Medium

## Summary
Add a visual on-screen indicator to show that LiteClip is actively recording. Users currently have no immediate visual feedback beyond the tray icon, which is invisible during fullscreen gaming.

## Current State
- No visual indicator exists on screen
- Recording state is only visible via tray icon and toast notifications
- Users in fullscreen games cannot see the system tray
- No confidence mechanism exists for "is it recording?"

## Implementation Steps

### 1. Indicator Design
- Small, configurable overlay element (red dot, text label, or both)
- Position: configurable corner (top-left, top-right, bottom-left, bottom-right)
- Size: small, medium, large options
- Opacity: configurable (semi-transparent to not obstruct gameplay)
- Show recording duration (e.g., "REC 02:34")

### 2. Overlay Window
- Create a transparent, click-through window using Win32 APIs
- Positioned on top of all windows (`HWND_TOPMOST`)
- Excluded from DXGI capture (use `SetWindowDisplayAffinity` with `WDA_EXCLUDEFROMCAPTURE`)
- Minimal performance impact (tiny window, simple rendering)

### 3. Configuration
- Add `RecordingIndicatorConfig` to config:
  - `enabled: bool`
  - `position: TopLeft | TopRight | BottomLeft | BottomRight`
  - `size: Small | Medium | Large`
  - `opacity: f32` (0.0-1.0)
  - `show_duration: bool`
  - `margin_px: u32`

### 4. State Management
- Show indicator when replay buffer is active
- Update duration text every second
- Hide when recording is paused or stopped
- Animate (pulse) for attention when recording starts

### 5. GUI
- Add indicator settings to the General or Advanced settings tab
- Preview the indicator position and style in settings

## Files to Modify
- `crates/liteclip-core/src/config/config_mod/types.rs` — Add recording indicator config
- `src/gui/` — New `recording_indicator.rs` module
- `src/main.rs` — Spawn indicator window, manage lifecycle
- `src/platform/` — Win32 overlay window creation

## Estimated Effort
Medium (3-5 days)

## Dependencies
- None (uses existing Win32 APIs)

## Risks
- `WDA_EXCLUDEFROMCAPTURE` requires Windows 10 2004 or later
- Overlay window must be carefully managed to not interfere with game input
- Performance impact must be negligible (simple GDI rendering)
