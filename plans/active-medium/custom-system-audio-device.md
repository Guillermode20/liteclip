# Plan: Custom System Audio Device Selection

## Status
Pending

## Priority
Medium

## Summary
Allow users to select a specific system audio output device for capture instead of always using the default render endpoint. Currently `system.rs:152` logs a warning that `custom device_id is not implemented yet; using default render endpoint`.

## Current State
- `AudioConfig` has fields for `mic_device` (string endpoint ID) for microphone selection
- No equivalent field exists for system audio device selection
- `WASAPISystemAudio` always uses the default render endpoint
- Device enumeration infrastructure exists for microphone selection

## Implementation Steps

### 1. Configuration
- Add `system_audio_device: String` to `AudioConfig` (endpoint ID, empty = default)
- Add `system_audio_device_name: String` for display purposes
- Serialize/deserialize in TOML config

### 2. Device Enumeration
- Enumerate all audio render (output) endpoints using WASAPI
- Filter to active, non-excluded devices
- Return (endpoint_id, friendly_name) pairs for UI display

### 3. Capture Integration
- Update `WASAPISystemAudio::new()` to accept an optional device ID
- If device ID is provided and valid, use it; otherwise fall back to default
- Handle device disconnection gracefully (fall back to default, show notification)

### 4. GUI
- Add device dropdown to the Audio settings tab (alongside existing mic device selector)
- Show friendly names, refresh on settings open
- Indicate which is the system default device

### 5. Device Change Events
- Subscribe to WASAPI device change notifications
- Auto-switch if the selected device is disconnected
- Revert to default if the preferred device becomes unavailable

## Files to Modify
- `crates/liteclip-core/src/config/config_mod/types.rs` — Add system audio device fields
- `crates/liteclip-core/src/capture/audio/system.rs` — Accept and use custom device ID
- `crates/liteclip-core/src/capture/audio/device_info.rs` — Add render device enumeration
- `src/gui/settings.rs` — Add system audio device selector UI

## Estimated Effort
Medium (2-3 days)

## Dependencies
- None (uses existing WASAPI infrastructure)

## Risks
- Device enumeration must handle cases where no output devices exist
- Device change events require COM apartment management on the correct thread
- Some virtual audio devices (Voicemeeter, VB-Cable) may behave unexpectedly
