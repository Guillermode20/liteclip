# Plan: macOS and Linux Cross-Platform Support

## Status
Pending

## Priority
Low (long-term)

## Summary
LiteClip is currently Windows-only. This plan outlines the work required to support macOS and Linux platforms by abstracting platform-specific capture, audio, and integration code.

## Current State
- All capture code uses DXGI (Windows-only API)
- All audio code uses WASAPI (Windows-only API)
- Hotkey registration uses Windows message loop
- System tray uses Windows-specific implementation
- All platform code gated behind `#[cfg(windows)]`
- `liteclip-core` is designed as a reusable library -- good foundation for cross-platform

## Implementation Steps

### 1. Platform Abstraction Layer
- Define platform-agnostic traits for:
  - `ScreenCapture` -- frame acquisition
  - `AudioCapture` -- system audio and microphone
  - `HotkeyManager` -- global hotkey registration
  - `TrayManager` -- system tray integration
  - `AutoStart` -- login item management
- Move Windows implementations behind `#[cfg(windows)]` modules

### 2. macOS Capture
- **Screen**: Use ScreenCaptureKit (macOS 12.3+) or ReplayKit
- **Audio**: Use CoreAudio for system audio (requires audio loopback driver) and microphone
- **Hotkeys**: Use Carbon Event Manager or NSEvent
- **Tray**: Use `tray-icon` crate (already cross-platform)
- **Auto-start**: Use LaunchAgents plist

### 3. Linux Capture
- **Screen**: Use PipeWire + xdg-desktop-portal (Wayland) or X11 XDamage
- **Audio**: Use PipeWire or PulseAudio for system audio and microphone
- **Hotkeys**: Use X11 XGrabKey or libinput (Wayland)
- **Tray**: Use `tray-icon` crate with StatusNotifierItem (Wayland) or XEmbed (X11)
- **Auto-start**: Use XDG autostart `.desktop` files

### 4. GUI Adjustments
- egui is already cross-platform -- minimal changes needed
- Adjust native resolution detection per platform
- Handle platform-specific file paths and permissions

### 5. Build System
- Update `Cargo.toml` features per platform
- Add platform-specific FFmpeg builds (Linux: package manager, macOS: Homebrew or bundled)
- Update installer/packaging: `.dmg` for macOS, `.deb`/`.rpm`/AppImage for Linux

## Files to Modify
- `crates/liteclip-core/src/capture/` -- Add `macos/` and `linux/` modules
- `crates/liteclip-core/src/capture/audio/` -- Add platform-specific audio backends
- `src/platform/` -- Add `macos/` and `linux/` modules
- `src/main.rs` -- Platform-specific initialization
- `Cargo.toml` -- Platform-specific dependencies
- `installer/` -- Add macOS and Linux packaging

## Estimated Effort
Very Large (3-6 weeks per platform)

## Dependencies
- macOS: ScreenCaptureKit requires macOS 12.3+
- Linux: PipeWire is now standard on most modern distros
- FFmpeg builds for each platform

## Risks
- macOS system audio capture requires additional setup (BlackHole, Soundflower)
- Linux screen capture varies significantly between Wayland and X11
- Permission models differ greatly (macOS requires explicit screen recording permission)
- Testing matrix grows significantly
