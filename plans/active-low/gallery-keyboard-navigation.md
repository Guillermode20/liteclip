# Plan: Gallery Keyboard Navigation and Accessibility

## Status
Pending

## Priority
Low

## Summary
Improve keyboard navigation and accessibility in the gallery browser. Currently navigation is mouse-dependent with limited keyboard support (Ctrl+Plus/Minus for card size, Escape to exit selection). Arrow-key navigation, focus management, and screen reader support are missing.

## Current State
- Limited keyboard navigation: Ctrl+Plus/Minus, Escape
- No arrow-key navigation between cards
- No focus rings or visible focus indicators
- No screen reader support
- Keyboard-heavy users cannot efficiently browse clips

## Implementation Steps

### 1. Arrow Key Navigation
- Left/Right arrow keys to navigate between clip cards
- Up/Down arrow keys to navigate rows in grid view
- Home/End to jump to first/last clip
- Page Up/Page Down to scroll by page
- Enter to open clip in editor
- Delete to delete selected clip (with confirmation)

### 2. Focus Management
- Visible focus ring on the currently focused card
- Focus follows selection state
- Tab navigation between major UI sections (search, filters, grid, panels)
- Focus restoration when returning from editor

### 3. Keyboard Shortcuts
- `Ctrl+A` — Select all clips
- `Ctrl+D` — Deselect all
- `Ctrl+E` — Export selected clip(s)
- `Ctrl+F` — Focus search input
- `Delete` — Delete selected clip(s)
- `Space` — Toggle clip selection
- `?` — Show keyboard shortcuts overlay

### 4. Accessibility
- Add descriptive labels to all interactive elements
- Ensure sufficient color contrast for text and UI elements
- Support Windows High Contrast mode
- Announce clip count and selection changes via screen reader

### 5. Configuration
- Add accessibility settings to General or Advanced tab:
  - `focus_ring_thickness: u32`
  - `keyboard_navigation_enabled: bool`
  - `high_contrast_mode: bool`

## Files to Modify
- `src/gui/gallery/browser.rs` — Keyboard navigation handlers
- `src/gui/gallery.rs` — Focus management, card selection state
- `src/gui/manager.rs` — Accessibility tree integration with egui
- `src/gui/settings.rs` — Accessibility settings

## Estimated Effort
Medium (3-5 days)

## Dependencies
- egui accessibility primitives

## Risks
- egui's accessibility support is still evolving
- Focus management in a virtualized grid is complex
- Screen reader support on Windows depends on egui's backend
