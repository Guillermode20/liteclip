# Plan: Clipboard Integration

## Status
Pending

## Priority
Medium

## Summary
Add clipboard integration to speed up clip sharing. After saving a clip, automatically copy the file path to the clipboard. Add a "Copy Path" button to gallery clip cards for quick access.

## Current State
- After saving a clip, the file path is logged but not copied to clipboard
- Users must manually navigate to the save directory to find clips
- Gallery has no copy-to-clipboard functionality
- Most common post-save action is sharing the clip

## Implementation Steps

### 1. Auto-Copy on Save
- Add `copy_path_to_clipboard: bool` to `GeneralConfig` (default: `true`)
- After clip save completes, write the absolute file path to the system clipboard
- Show toast notification: "Clip saved! Path copied to clipboard."

### 2. Gallery Copy Button
- Add a clipboard icon button to each clip card in the gallery
- On click: copy the clip's file path to clipboard
- Show brief toast confirmation: "Path copied"

### 3. Copy File (Optional)
- Add option to copy the actual file (not just the path) to clipboard
- Enables direct paste into Discord, email, or file explorer
- Use `CF_HDROP` format for Windows file clipboard

### 4. Configuration
- Add clipboard settings to General tab:
  - `copy_path_on_save: bool`
  - `copy_file_on_save: bool` (mutually exclusive with path copy)

### 5. Error Handling
- Clipboard may be locked by another application
- Retry with brief delay, then fall back to toast-only notification
- Never block the save pipeline on clipboard operations

## Files to Modify
- `crates/liteclip-core/src/config/config_mod/types.rs` — Add clipboard config fields
- `src/main.rs` — Clipboard write after clip save in `spawn_save_clip_task`
- `src/gui/gallery/browser.rs` — Copy path button on clip cards
- `src/platform/` — Windows clipboard API wrapper

## Estimated Effort
Small (1-2 days)

## Dependencies
- None (uses Win32 clipboard APIs)

## Risks
- Clipboard is a shared resource; contention with other apps
- Copying large files to clipboard may be slow
- Some applications may not accept file drops from clipboard
