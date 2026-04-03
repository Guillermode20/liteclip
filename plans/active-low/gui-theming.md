# Plan: GUI Theming and Customization

## Status
Pending

## Priority
Low

## Summary
Add theme customization to the LiteClip GUI. Currently the egui interface uses the default theme with no customization options. Users should be able to choose between light/dark themes and customize accent colors.

## Current State
- egui uses its default dark theme
- No theme selection or customization options
- No accent color configuration
- All UI colors are hardcoded or use egui defaults

## Implementation Steps

### 1. Theme Presets
- Define built-in theme presets:
  - **Dark** (default, current)
  - **Light** (for daytime use)
  - **OLED** (pure black background)
  - **System** (follow Windows theme setting)

### 2. Accent Color
- Allow custom accent color selection
- Color picker widget in settings
- Accent color applies to buttons, links, selection highlights, and progress bars
- Provide preset accent colors (blue, green, purple, orange, red)

### 3. Theme Configuration
- Add `ThemeConfig` to config:
  - `theme: Dark | Light | OLED | System`
  - `accent_color: Option<[u8; 4]>` (RGBA, None = default)
  - `ui_scale: f32` (0.8-1.5, for high-DPI displays)
  - `compact_mode: bool` (reduced padding and spacing)

### 4. System Theme Detection
- Detect Windows light/dark mode setting via registry
- Update theme automatically when system setting changes
- Allow manual override

### 5. GUI
- Add "Appearance" section to General settings tab
- Theme selector with live preview
- Accent color picker with presets
- UI scale slider
- Compact mode toggle

### 6. Persistence
- Save theme preferences to config
- Apply theme on startup
- Smooth transition when theme changes (no full restart needed)

## Files to Modify
- `crates/liteclip-core/src/config/config_mod/types.rs` — Theme config struct
- `src/gui/manager.rs` — Theme application logic
- `src/gui/settings.rs` — Appearance settings section
- `src/gui/gallery.rs` — Respect theme in gallery UI
- `src/main.rs` — System theme detection

## Estimated Effort
Medium (3-5 days)

## Dependencies
- Windows registry access for system theme detection

## Risks
- egui theme customization is limited compared to web frameworks
- Some UI elements may not respect custom colors
- System theme detection requires polling (no event on Windows)
