# Plan: Gallery Video Metadata Panel

## Status
Pending

## Priority
Low

## Summary
Add a detailed metadata panel to the gallery showing codec, bitrate, audio channels, sample rate, encoder used, and other technical details for each clip. Currently only duration, size, and resolution are shown on clip cards.

## Current State
- Gallery cards show duration, size, and resolution
- No detailed metadata panel exists
- Users must use external tools (MediaInfo, FFprobe) for technical details
- Troubleshooting quality issues requires external analysis

## Implementation Steps

### 1. Metadata Extraction
- Use FFmpeg to extract detailed metadata from each clip:
  - Video: codec, profile, level, bitrate, framerate, keyframe interval, color space
  - Audio: codec, channels, sample rate, bitrate, language
  - Container: format, duration, creation time
  - LiteClip: encoder used, settings at time of recording

### 2. Metadata Storage
- Cache metadata in the companion file cache
- Extract on first gallery open or when clip is added
- Invalidate cache when clip is modified or exported

### 3. Metadata Panel UI
- Right-click context menu on clip cards: "Properties" or "Metadata"
- Side panel or modal dialog with organized sections:
  - Video, Audio, Container, Recording Settings
- Copy metadata as JSON or text button

### 4. Gallery Card Enhancement
- Show codec badge on clip cards (e.g., "HEVC", "H.264")
- Show audio channel count badge (e.g., "Stereo")
- Color-code badges for quick identification

### 5. Batch Metadata Export
- Option to export metadata for all clips as CSV or JSON
- Useful for inventory, analysis, or external processing

## Files to Modify
- `src/gui/gallery/browser.rs` — Context menu and metadata panel
- `src/gui/gallery.rs` — Metadata panel component
- `crates/liteclip-core/src/output/video_file.rs` — Extended metadata struct and extraction
- `crates/liteclip-core/src/output/companion_cache.rs` — Cache metadata

## Estimated Effort
Small (1-2 days)

## Dependencies
- FFmpeg probe (already available)

## Risks
- Metadata extraction adds latency on gallery open
- Some clips may have incomplete or missing metadata
- Cache invalidation must handle external file modifications
